//! The drive-delivery seam: how a finished drive RECORD reaches the plant and how
//! the response record comes back. This is what keeps `eigen-twr` hardware-
//! agnostic.
//!
//! # Why the law never touches a backend
//!
//! TWR's control law runs BETWEEN plays: it consumes (target, achieved) records
//! and produces the next drive record. Only the delivery of a finished record
//! varies per backend, so that is the whole trait surface:
//!
//! - [`BackendReferencePlayer`] - the ONE production impl: plays
//!   the record sample-by-sample through any [`eigen_hal::Backend`]
//!   (`write_drive` one frame, `read_input` one frame - the exact
//!   `BackendPulseFirer` shape the shock services proved). Works against
//!   eigen-hal-sim today and against direct-DAC / NI
//!   backends when they land, with zero TWR changes: new hardware implements
//!   `Backend`, not this trait.
//! - The delegated (reference-download) player is a documented seam, not code:
//!   on a delegated controller every mode is host-downloads-reference + GO,
//!   so `play()` maps to: download
//!   the drive record via the delegated backend's byte-verified reference
//!   builders, GO, read the
//!   telemetry stream back (single-channel 5120 Hz f32, decoded 2026-07-04),
//!   return it. It is NOT built here because live firing is blocked on the
//!   shared mode-entry/arm capture. NOTE the honesty
//!   twist unique to TWR: even on the delegated path the ITERATION still runs
//!   host-side between plays - TWR's law survives delegation because it operates
//!   BETWEEN plays, not inside them.
//!
//! # Why NOT `eigen_engine::Engine`
//!
//! TWR v1 is single-control-channel and plays finished records directly against
//! the `Backend` (as the shock firer does), which is the right shape for a
//! play-then-iterate module: there is no per-tick control law to run, so the
//! engine's tick machinery buys nothing. TWR-MIMO revisits this before
//! multi-control lands.
//!
//! The two reasons originally recorded here are BOTH retired:
//!
//! * The engine's frames-vs-samples contract bug was fixed 2026-07-16.
//! * The `SimBackend` per-block drive latch, which forced drive to be fed ONE
//!   FRAME AT A TIME, was removed - the Sim now buffers the full per-frame drive
//!   stream.
//!
//! So playback is now BLOCK-WISE through [`eigen_hal::play_block_wise`]: write a
//! block honoring `write_drive`'s accepted count and retain the remainder, then
//! read that many frames back with bounded `NoDataYet` polling.
//! One `write_drive` per sample was never what a real driver wants.
//!
//! # The per-play task cycle (the shock-firer shape)
//!
//! The run's setup and the play's task lifetime are SPLIT:
//!
//! * [`BackendReferencePlayer::open_and_configure`] runs ONCE per run - open,
//!   configure, adopt the rate, consult the arm gate, derive the width and the
//!   lag baseline, verify bring-up, then PARK. Nothing starts here. Configure
//!   per play would re-derive `block_frames` and the lag on every pass, so a
//!   device that answered slightly differently on play 7 would silently
//!   re-align the drive/response pairing mid-run.
//! * `play()` runs the cycle: `arm` -> commit what the device will take BEFORE
//!   `start` -> `start` -> stream the remainder block-wise -> capture -> `stop`
//!   (park + unreserve).
//!
//! What the cycle buys, stated honestly: a clean HEAD (the first frames
//! clocked out are real drive, not the backend's start-time silence top-up),
//! and an inter-play host-compute gap that is a non-event because the task is
//! parked between plays. **It does NOT remove the underrun class.** A TWR
//! record is 10^5-10^6 frames against an AO buffer of a few thousand, so most
//! of every record STREAMS after `start` and an underrun there is
//! `RampDownThenAbort` exactly as before (an earlier "no underrun class"
//! claim is deleted, not implemented). The streaming window is
//! `record + pad - committed` frames.
//!
//! # Rest at zero is CONSTRUCTED, not assumed
//!
//! Stopping an AO task does not zero the pin: the 9263 holds its last clocked
//! sample and does not support `AO.IdleOutputBehavior = ZeroVolts`. The HAL's
//! `stop()` appends a 250 ms raised-cosine taper unless the last written frame
//! is BIT-EXACTLY `0.0`, so every play's buffer ends with an EXACT-ZERO PAD
//! (see [`BackendReferencePlayer::rest_pad_frames`]) - a pad, never a taper of
//! the record. The happy path then skips the taper and the per-play cost stays
//! bounded; a play cut short mid-record keeps the taper, which is what brings
//! the pin down there.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eigen_channel_table::{ChannelRole, ChannelTable};
use eigen_hal::{Backend, HalError, StreamError};
use serde::{Deserialize, Serialize};

use crate::TwrError;

/// Which delivery path a TWR run drives. TWR-local (deliberately NOT
/// `eigen_shock_common::path::DrivePath`: shock's capability table is
/// shock-semantic; TWR's honesty facts differ - see [`TwrPathCapabilities`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TwrPath {
    /// The host plays the drive record through an `eigen_hal::Backend`
    /// (eigen-hal-sim today; direct-DAC / NI later).
    HostBackend,
    /// A delegated controller replays a downloaded reference (the reference-download-shaped
    /// delegated path). Seam only.
    Delegated,
}

/// Capability truth per path, decided in ONE place (the shock `PathCapabilities`
/// pattern). The TWR-specific fact: the correction law runs host-side on BOTH
/// paths, because it operates between plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TwrPathCapabilities {
    /// The iterative correction law runs (host-side, between plays). TRUE on
    /// both paths - the TWR honesty twist.
    pub adaptation_runs_hostside: bool,
    /// The response record is observable, so the loop can actually close.
    /// FALSE on the delegated path until the telemetry/arm capture gap closes;
    /// without a response the run can download + play but not iterate.
    pub response_observable: bool,
    /// Per-sample drive clipping is enforceable at the point of playout.
    pub drive_clip_enforceable: bool,
}

impl TwrPathCapabilities {
    pub const fn for_path(path: TwrPath) -> Self {
        match path {
            TwrPath::HostBackend => TwrPathCapabilities {
                adaptation_runs_hostside: true,
                response_observable: true,
                drive_clip_enforceable: true,
            },
            TwrPath::Delegated => TwrPathCapabilities {
                adaptation_runs_hostside: true,
                response_observable: false,
                drive_clip_enforceable: false,
            },
        }
    }
}

/// The drive-delivery seam (see the module docs).
pub trait ReferencePlayer {
    /// Which delivery path this player drives (honesty discriminant).
    fn path(&self) -> TwrPath;

    /// Playout sample rate, Hz - the rate a caller can ask for BEFORE arming,
    /// i.e. the REQUESTED rate. The prepared reference is built against it (the
    /// service validates; the RESAMPLE prep op is the explicit remedy).
    fn fs_hz(&self) -> f64;

    /// The rate the hardware actually programmed, once it has said. `None`
    /// before the path is up, or on a path that cannot state one.
    ///
    /// This is the number that can DISAGREE with [`ReferencePlayer::fs_hz`] -
    /// the 9234 coerces a request up its own ladder - and the disagreement is
    /// what the arm gate refuses on. Status shows both.
    fn actual_fs_hz(&self) -> Option<f64> {
        None
    }

    /// State the rate the PREPARED reference record carries, so the arm gate
    /// has a claim to check.
    ///
    /// The service calls this on construction, ALWAYS. Without it the rate arm
    /// of the gate is vacuously green (`arm_gate` skips the check on `None`) -
    /// and TWR is the one module where the reference record is a genuine second
    /// rate authority, so a vacuous gate here means a whole test played at the
    /// wrong speed with nothing refusing.
    fn set_reference_rate_hz(&mut self, _hz: f64) {}

    /// Where the drive→response alignment in force came from, where the path
    /// aligns at all. `None` means "this path does not align host-side" (a
    /// delegated path returns device-aligned records), and the service treats
    /// it as no claim either way.
    ///
    /// The service refuses to proceed past sysid on
    /// [`LagSource::DerivedNoPeak`]: a constant misalignment is a LINEAR phase
    /// and Welch coherence cannot see it, so the coherence gate downstream
    /// would bless an H whose phase is wrong by exactly the fallback error —
    /// the 2026-07-30 collapse with a green gate in front of it.
    fn lag_provenance(&self) -> Option<LagSource> {
        None
    }

    /// The drive→response alignment in force, FRAMES. `None` = this path does
    /// not align host-side (see [`ReferencePlayer::lag_provenance`]).
    ///
    /// # Why a host needs the number and not only its provenance
    ///
    /// The provenance says WHERE the alignment came from; the daemon's status
    /// surface has to say WHAT it is. A run reporting `Measured` and a run
    /// reporting `Measured` at 1449 frames are the same sentence to an operator
    /// and different facts to anyone diagnosing a collapsed coherence, and the
    /// number is the half that identifies the plant's actual transport delay.
    fn lag_frames(&self) -> Option<usize> {
        None
    }

    /// Frames the device ACCEPTED before `start()` on the last play - the
    /// commit depth. `None` = this path does not commit ahead of the clock.
    ///
    /// The rest of the record STREAMED, so `record + pad - committed` is the
    /// underrun window, which is the one number that makes a
    /// `RampDownThenAbort` explicable rather than mysterious.
    fn committed_frames(&self) -> Option<usize> {
        None
    }

    /// Drain the DRIVER's own diagnostics (underruns, rate coercions) since the
    /// last call, so a host can journal them.
    ///
    /// TAKE, not peek: a warning collected on a
    /// backend and never drained is a signal that reaches nobody, and a peeking
    /// accessor makes the caller invent a watermark - which is the count-based
    /// scheme `sine.rs`'s `fresh_driver_warnings` exists to avoid.
    fn take_driver_warnings(&mut self) -> Vec<String> {
        Vec::new()
    }

    /// Bring the path up: open, configure, adopt the rate, consult the arm
    /// gate. Called ONCE per run.
    ///
    /// **Nothing starts here.** The task lifetime is per PLAY (see the module
    /// docs); a path that parks between plays leaves this call holding only the
    /// facts that must not change mid-run.
    fn arm(&mut self) -> Result<(), TwrError>;

    /// Deliver the FULL drive record, play it once, and return the captured
    /// control-channel response record - the SAME length as `drive`, aligned to
    /// playout start (sample `i` of the response is the plant output while
    /// drive sample `i` was commanded). `Some(response)` where the response is
    /// observable; `None` where it is not (delegated telemetry gap) - never a
    /// fabricated record.
    ///
    /// Returns [`TwrError::Cancelled`] when the operator's cancel flag was seen
    /// at a block boundary - a distinct outcome from an abort, and the caller's
    /// to map.
    fn play(&mut self, drive: &[f64]) -> Result<Option<Vec<f64>>, TwrError>;

    /// The station class this path is bound to, for the honesty label.
    /// `None` = the path cannot name one (a scripted double, a seam).
    fn station_label(&self) -> Option<String> {
        None
    }

    /// Take whatever the display tap has collected since the last drain.
    /// `None` = no tap on this player.
    ///
    /// Drained BETWEEN units by the caller, never from inside a play: the tap
    /// exists so a host can show live samples without the play having to know
    /// about a channel, a lock or a consumer.
    fn drain_display_tap(&mut self) -> Option<TapDrain> {
        None
    }

    /// Ask an in-flight `play()` to stop at its next block boundary.
    ///
    /// Takes `&self` because that is the whole point: the flag it raises is
    /// shared, so a command thread can raise it WITHOUT the lock the run thread
    /// holds across the play. Raising it only after taking that lock is what
    /// made Stop wait out the rest of the record.
    fn request_cancel(&self) {}

    /// Tear the path down (backend stop) - the RAMPED path, 250 ms taper.
    fn stop(&mut self) -> Result<(), TwrError>;

    /// Tear the path down IMMEDIATELY (25 ms taper, terminal-then-rebuild).
    /// Defaults to [`ReferencePlayer::stop`] for a path with no faster route.
    fn emergency_stop(&mut self) -> Result<(), TwrError> {
        self.stop()
    }
}

fn hal_err(e: HalError) -> TwrError {
    TwrError::Play(e.to_string())
}

/// Default quiet frames appended AFTER each record so the plant rings down
/// before the next play begins (the between-plays half; the grid's own pads
/// handle ring-down WITHIN a play).
///
/// This is the length the deleted pre-play flush used, kept deliberately: the
/// flush's JOB was right and its POSITION was wrong. Ahead of the record it
/// pushed the leading edge behind a lookahead of silence, which breaks the rule
/// that "the first thing written after the clock starts must be real drive".
/// Behind the record it costs the same wall time, keeps consecutive plays
/// independent, and doubles as the exact-zero rest pad.
pub const DEFAULT_QUIET_FRAMES: usize = 1024;

/// The host player over any [`Backend`] (see the module docs).
pub struct BackendReferencePlayer<B: Backend> {
    backend: B,
    table: ChannelTable,
    /// The station class the arm gate is given. Supplied
    /// by whoever built the backend - the UI's station factory in a run, the test
    /// file in a fixture - because the class is a property of what the backend is
    /// bound to, not of `B`.
    station: eigen_module_abi::StationClass,
    /// Interleaved input frame width: enabled non-Drive channels, table order.
    n_inputs: usize,
    /// Slot (within the input frame) of the first enabled Control channel.
    control_slot: usize,
    /// Quiet ring-down frames appended after each record.
    quiet_frames: usize,
    /// Enabled DRIVE rows in the table. One today; more than one is
    /// refused at `open_and_configure` rather than written mono.
    n_drive: usize,
    /// The rate the prepared reference RECORD was generated at, when the caller
    /// states it. `None` = not stated, and the arm gate then has no claim to
    /// check - it must never invent one. Set it via
    /// [`BackendReferencePlayer::with_reference_rate_hz`] and the gate refuses a
    /// record whose rate disagrees with the rate the hardware configured,
    /// rather than playing it at the wrong speed.
    reference_rate_hz: Option<f64>,
    /// The path is open + configured + gated (once per run). NOT "a task is
    /// running" - tasks live for one play.
    configured: bool,
    /// The rate the backend answered at configure. `None` until then.
    actual_fs_hz: Option<f64>,
    /// Frames the response trails the drive by on this delivery path. Set to
    /// the DERIVED baseline at configure and replaced by the xcorr measurement
    /// off the first (sysid) record; see [`LagSource`].
    pipeline_lag_frames: usize,
    lag_source: LagSource,
    /// The device's declared output lookahead, cached at configure so a play
    /// never re-queries caps.
    lookahead_frames: usize,
    /// Frames per write/read block, from what the backend CONFIGURED. Set in
    /// `arm()`; 1 until then.
    block_frames: usize,
    /// The block the player REQUESTS at configure. `None` = derive it from the
    /// backend's rate (`eigen_hal::default_block_frames`), which is what a
    /// hardware station wants; a caller with a reason overrides it.
    requested_block: Option<usize>,
    /// Reusable input scratch, sized at arm so a play allocates nothing.
    in_scratch: Vec<f32>,
    /// Reusable interleaved drive block (f64 record -> f32 frames).
    drive_scratch: Vec<f32>,
    /// The operator cancel flag, cached at construction and handed to
    /// `play_block_wise`. Shared so a command thread can raise it while the run
    /// thread is inside a play.
    ///
    /// There is ONE call site now, not the two originally expected: the second
    /// was the pre-play flush, and the flush is gone (its quiet window moved to
    /// the record's tail). A cancel flag on a call that no longer exists would
    /// have been a wire to nowhere.
    cancel: Arc<AtomicBool>,
    /// Frames the device accepted before `start()` on the last play (the
    /// commit depth `play_block_wise` is seeded with).
    last_committed: usize,
    /// The display tap (off unless a caller asks for one).
    ///
    /// SHARED since the daemon host landed, and that is what makes daemon-side time streaming
    /// possible at all. The tap used to be a plain field drained through
    /// `&mut self`, so the only way to read it was to hold the service - and
    /// the daemon's pump holds the service for the WHOLE of a play. A 30 s
    /// record would therefore have published nothing until it finished, which
    /// is the opposite of what a live time pane is for. Behind an
    /// `Arc<Mutex<..>>` a second thread drains it at wall-clock cadence while
    /// the play is still running.
    ///
    /// The lock is taken ONCE PER BLOCK, never per sample: the play closure
    /// pushes a whole block under one uncontended acquisition, which is a
    /// handful of nanoseconds against a block that is tens of milliseconds of
    /// wall time. Per-sample locking would have put ~51 200 acquisitions per
    /// second on the thread that is feeding the device.
    tap: Option<Arc<Mutex<SampleRing>>>,
    /// The per-block PROGRESS observer a host installs to watch a play from
    /// outside the service mutex ([`PlayObserver`]).
    play_observer: Option<PlayObserver>,
}

/// How far through one play the device has got, reported once per block.
///
/// `frames_expected` counts the record PLUS its exact-zero rest pad, because
/// that is what the device is actually clocking - a progress readout that
/// stopped at the record's end would sit at 100% through the pad and then keep
/// going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayProgress {
    /// Frames read back so far this play.
    pub frames_done: usize,
    /// Frames this play will read back in total (record + rest pad).
    pub frames_expected: usize,
}

/// A host's per-block hook into a play. See
/// [`BackendReferencePlayer::with_play_observer`].
///
/// `Fn` + `Send` + `Sync` rather than `FnMut`: it is called from the play loop
/// and shared with a status path, so it must not need exclusive access. Hosts
/// store their counters in atomics, which is what makes the status surface
/// lock-free.
pub type PlayObserver = Arc<dyn Fn(PlayProgress) + Send + Sync>;

/// Where [`BackendReferencePlayer::pipeline_lag_frames`] came from. The
/// distinction is load-bearing: the DERIVED number undershot by 1154 frames on
/// this device family, which
/// is why the measurement exists at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LagSource {
    /// `min_lookahead_frames + group_delay_frames` at configure. The baseline
    /// the first play runs on, because nothing has been measured yet.
    Derived,
    /// Measured by cross-correlation against the first (sysid) record.
    Measured,
    /// A measurement was attempted and found NO peak above the noise floor, so
    /// the derived baseline stands. Distinct from `Derived` on purpose: "the
    /// search could have produced a number and did not" is a finding, not a
    /// default.
    DerivedNoPeak,
}

impl<B: Backend> BackendReferencePlayer<B> {
    /// Build a player over `backend` with `table`, armed against `station`. The
    /// control channel is the FIRST enabled `Control`-role channel (slot 0
    /// fallback when none is marked). Fails on an invalid table before any
    /// backend traffic.
    pub fn new(
        backend: B,
        table: ChannelTable,
        station: eigen_module_abi::StationClass,
    ) -> Result<Self, TwrError> {
        table
            .validate()
            .map_err(|e| TwrError::Config(format!("channel table: {e}")))?;
        let mut n_inputs = 0usize;
        let mut n_drive = 0usize;
        let mut control_slot = None;
        for ch in table.channels.iter().filter(|c| c.enabled) {
            if ch.role == ChannelRole::Drive {
                n_drive += 1;
                continue;
            }
            if ch.role == ChannelRole::Control && control_slot.is_none() {
                control_slot = Some(n_inputs);
            }
            n_inputs += 1;
        }
        if n_inputs == 0 {
            return Err(TwrError::Config(
                "channel table has no enabled input channels".into(),
            ));
        }
        if n_drive == 0 {
            return Err(TwrError::Config(
                "channel table has no enabled Drive channel; a record player with no drive \
                 path has nothing to play"
                    .into(),
            ));
        }
        Ok(BackendReferencePlayer {
            backend,
            table,
            station,
            n_inputs,
            control_slot: control_slot.unwrap_or(0),
            quiet_frames: DEFAULT_QUIET_FRAMES,
            n_drive,
            reference_rate_hz: None,
            configured: false,
            actual_fs_hz: None,
            pipeline_lag_frames: 0,
            lag_source: LagSource::Derived,
            lookahead_frames: 0,
            block_frames: 1,
            requested_block: None,
            in_scratch: Vec::new(),
            drive_scratch: Vec::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            last_committed: 0,
            tap: None,
            play_observer: None,
        })
    }

    /// State the rate the prepared reference record was generated at, so the
    /// arm gate can refuse a record that does not match the configured hardware
    /// rate. Without it the gate has no claim to check.
    pub fn with_reference_rate_hz(mut self, hz: f64) -> Self {
        self.reference_rate_hz = Some(hz);
        self
    }

    /// Set the quiet ring-down window appended after each record
    /// ([`DEFAULT_QUIET_FRAMES`]).
    pub fn with_quiet_frames(mut self, n: usize) -> Self {
        self.quiet_frames = n;
        self
    }

    /// Share this player's cancel flag with a caller that must be able to stop
    /// a play from another thread (the UI's `twr_stop`, which must raise it
    /// BEFORE it takes the service lock the run thread is holding).
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// Attach a bounded display tap of `capacity` control-channel samples.
    ///
    /// OFF by default, because a tap nobody drains is a ring that fills and
    /// then reports a drop count nobody reads. This is the precondition for
    /// daemon-side time streaming: the play collects, the caller drains between
    /// units, and the RT path never learns who the consumer is.
    pub fn with_display_tap(mut self, capacity: usize) -> Self {
        self.tap = Some(Arc::new(Mutex::new(SampleRing::new(capacity))));
        self
    }

    /// The SHARED handle to this player's display tap, or `None` when no tap
    /// was asked for.
    ///
    /// Taken BEFORE the player is moved into a [`crate::TwrService`], because
    /// the service owns it by value afterwards. A host that holds this handle
    /// can drain the ring on its own thread while a play is in flight - which
    /// is the whole reason the ring is shared (see
    /// [`BackendReferencePlayer::tap`]).
    pub fn display_tap_handle(&self) -> Option<Arc<Mutex<SampleRing>>> {
        self.tap.clone()
    }

    /// Install a per-block PROGRESS observer for a host that must report on a
    /// play it cannot see into.
    ///
    /// # Why this exists and a poll does not serve
    ///
    /// A play holds the service (and therefore the host's mutex) for its whole
    /// duration. For a 30 s TWR record that means a status poll has nothing new
    /// to say for 30 s: no frame count, and no evidence that the driver is
    /// still returning blocks at all. The observer is the only place inside the
    /// play a host can be handed those two facts, and it hands over NOTHING
    /// ELSE - no samples, no locks, no allocation - so it cannot become a
    /// second display path competing with the tap.
    pub fn with_play_observer(mut self, obs: PlayObserver) -> Self {
        self.play_observer = Some(obs);
        self
    }

    /// Adopt a cancel flag the CALLER already holds, instead of handing one
    /// back. Same channel, opposite direction: a host that owns the flag for
    /// the life of its glue (so no lock stands between an operator Stop and the
    /// flag) hands it to each player it builds.
    pub fn with_cancel_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancel = flag;
        self
    }

    /// Override the acquisition block this player REQUESTS at configure. The
    /// block it actually runs still comes from `ConfiguredActual`; this only
    /// changes the request.
    pub fn with_block_frames(mut self, frames: usize) -> Self {
        self.requested_block = Some(frames.max(1));
        self
    }

    /// The block this player is streaming at (valid after `arm()`).
    pub fn block_frames(&self) -> usize {
        self.block_frames
    }

    /// Frames the response trails the drive by on this path (valid after
    /// `arm()`). Exposed because it is the one number the drive/response
    /// pairing rests on, and a fixture cannot check an alignment it cannot see.
    pub fn pipeline_lag_frames(&self) -> usize {
        self.pipeline_lag_frames
    }

    /// Where the lag in force came from. A run that reports `DerivedNoPeak` is
    /// aligned on a formula the device family is known to undershoot.
    pub fn lag_source(&self) -> LagSource {
        self.lag_source
    }

    /// Frames the device took before `start()` on the last play. The rest of
    /// the record STREAMED (the underrun window).
    pub fn last_committed_frames(&self) -> usize {
        self.last_committed
    }

    /// The exact-zero pad appended to every played record: the device's own
    /// lookahead plus one block, or the quiet ring-down window, whichever is
    /// longer.
    ///
    /// THREE jobs, and the max is how one pad serves all three.
    ///
    /// 1. The LAST frame the device clocks is a bit-exact `0.0`, so `stop()`'s
    ///    zero-guard fires and the play does not pay the 250 ms taper - which
    ///    needs the pad to cover the device's whole output buffer.
    /// 2. It is the search bound for the measured lag: a response can only be
    ///    trimmed by `lag` frames and still yield a full record if the buffer
    ///    carried `lag` extra frames, so the pad is exactly how much delay this
    ///    player can absorb.
    /// 3. It rings the plant down between plays ([`DEFAULT_QUIET_FRAMES`]).
    ///    Job 1's length is set by the AO buffer and has nothing to do with the
    ///    plant, so on a station with a short buffer it is NOT enough on its
    ///    own: dropping to `lookahead + block` alone left the sim_e2e plant
    ///    still ringing from the 8 s sysid record when the first iteration
    ///    began, and the run tripped the response envelope 18 samples in.
    pub fn rest_pad_frames(&self) -> usize {
        (self.lookahead_frames + self.block_frames.max(1)).max(self.quiet_frames)
    }

    /// The wrapped backend (post-run inspection in tests).
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Commit as much of `drive_scratch` as the device will take while the task
    /// is NOT running. Returns the frames it took.
    ///
    /// # `Some(0)` before the first write is the UNKNOWN, not "full"
    ///
    /// `Backend::output_space` renders a failed device query as `Some(0)` on
    /// purpose: for a RUNNING task the conservative answer is "do not top up".
    /// This runs on a task that was just armed from parked, whose buffer is
    /// empty by construction, so `Some(0)` here can only be the unknown. Read
    /// as "full" it makes this function a no-op, `start()` then finds an empty
    /// buffer and tops it up with a full lookahead of SILENCE, and the record
    /// plays a lookahead late behind it - the defect that cost shock a
    /// -2049-frame record shift. Before anything is accepted the space query is
    /// ADVISORY; the authority is `write_drive`'s own accept count.
    ///
    /// Pre-start, DAQmx ERRORS (-200293) on a write larger than the free
    /// buffer rather than accepting a prefix, so each write is capped at the
    /// reported space and a record longer than the buffer commits what fits.
    /// For TWR that is EVERY record: `play_block_wise` streams the remainder.
    fn commit_what_fits(&mut self) -> Result<usize, TwrError> {
        let n_drive = self.n_drive();
        let total = self.drive_scratch.len() / n_drive;
        let block = self.block_frames.max(1);
        let space_before = self.backend.output_space();
        let mut done = 0usize;
        while done < total {
            let space = match self.backend.output_space() {
                // No space report (Sim mocks with no output contract): offer
                // the remainder and stop on a zero accept.
                None => total - done,
                Some(0) if done == 0 => block.min(total),
                Some(0) => break,
                Some(s) => s.min(total - done),
            };
            if space == 0 {
                break;
            }
            let accepted = self
                .backend
                .write_drive(&self.drive_scratch[done * n_drive..(done + space) * n_drive])
                .map_err(hal_err)?;
            if accepted == 0 {
                break;
            }
            done += accepted.min(total - done);
        }
        self.last_committed = done;
        if std::env::var_os("EIGEN_NI_DIAG").is_some() {
            eprintln!(
                "[twr-diag] commit_what_fits: record {total} frames, space-before \
                 {space_before:?}, block {block} -> COMMITTED {done} frames before start(); \
                 {} frames STREAM after it",
                total - done
            );
        }
        Ok(done)
    }

    /// Drive channels per frame, DERIVED from the table.
    ///
    /// It was hard-coded `1` at both write sites. On a 2-drive table that makes
    /// `play_block_wise` treat the mono record as interleaved pairs: the record
    /// alternates across two DACs at half rate, each of them playing half the
    /// samples. The tell is nasty because it is not obviously broken - coherence
    /// stays high and |H| comes back 2x wrong - so the width is refused at
    /// bring-up instead (see `open_and_configure`) rather than written blind.
    fn n_drive(&self) -> usize {
        self.n_drive
    }

    /// ONCE per run: open, configure, ADOPT the rate, consult the arm gate,
    /// derive the lag baseline, verify bring-up, PARK.
    ///
    /// **Nothing starts here and no task is left up.** `arm()` then `stop()` is
    /// the driver-side "does this configuration commit" check; the state every
    /// play begins from is parked-and-unreserved.
    ///
    /// The earlier shape started the task here and prefilled a lookahead of
    /// SILENCE to satisfy an NI AO that refuses to start on an empty buffer
    /// (-200462). That put the silence exactly where the record's leading edge
    /// belonged and made the lag a function of it. The prefill is GONE: the
    /// per-play cycle commits real drive before `start`, which satisfies the
    /// same driver rule with the record instead of with silence ("the first thing
    /// written after the clock starts must be real drive").
    pub fn open_and_configure(&mut self) -> Result<(), TwrError> {
        self.backend.open().map_err(hal_err)?;
        let fs = self.backend.caps().sample_rate_hz;
        // The block is REQUESTED here and the backend's answer is what the play
        // actually runs at.
        let want_block = self
            .requested_block
            .unwrap_or_else(|| eigen_hal::default_block_frames(fs));
        let acq = eigen_hal::AcquisitionConfig::new(fs, want_block);
        let actual = self.backend.configure(&self.table, &acq).map_err(hal_err)?;

        // THE ADOPTION, before the gate: everything Hz-denominated downstream
        // reads back through `actual_fs_hz()`, and a rate nothing can be
        // denominated against is refused here rather than divided by later.
        if !(actual.actual_rate_hz.is_finite() && actual.actual_rate_hz > 0.0) {
            return Err(TwrError::Play(format!(
                "backend configured a non-usable rate ({} Hz); nothing can be denominated \
                 against it",
                actual.actual_rate_hz
            )));
        }
        self.actual_fs_hz = Some(actual.actual_rate_hz);

        // THE ARM GATE. TWR on a host backend requests
        // `{HostSynthesized, HostBetweenRuns}`: the host streams the drive
        // record AND its correction law runs BETWEEN plays. That pairing is the
        // whole reason the resolver has two axes - the three-valued form could
        // not name a mode where the host both plays and iterates on a timescale
        // of seconds. It carries NO shared-clock requirement (each play is
        // independent; there is no COLA continuity to preserve), which is what
        // keeps TWR-on-NI off the arrival-day clock bench result.
        //
        // The factory supplies the class; it is held on the player from
        // construction so the gate reads the SAME value the caller stated.
        eigen_module_abi::arm_gate(
            &self.backend.caps(),
            self.station,
            eigen_module_abi::Resolution::host_iterative(),
            // The reference RECORD is a sampled waveform, so it is genuinely
            // rate-bearing: playing it at a different rate rescales the whole
            // test in frequency. This is the module the rate policy exists for,
            // so this arm is never `None` in a run - the service states it.
            self.reference_rate_hz,
            actual.actual_rate_hz,
        )
        .map_err(|reason| TwrError::State(format!("arm refused: {reason}")))?;

        // THE WIDTH, refused loudly rather than written mono.
        if self.n_drive != 1 {
            return Err(TwrError::Config(format!(
                "the channel table has {} enabled Drive rows; TWR v1 plays ONE drive record \
                 and has no rule for splitting it across {} DACs. Writing it as if the \
                 table were mono would alternate the record across the outputs at half \
                 rate - which reads back as high coherence and an |H| that is wrong by the \
                 drive count, not as an error. Enable exactly one Drive row",
                self.n_drive, self.n_drive
            )));
        }

        self.block_frames = actual.block_frames.max(1);
        self.lookahead_frames = self
            .backend
            .caps()
            .output_buffer
            .map(|c| c.min_lookahead_frames as usize)
            .unwrap_or(0);

        // THE PIPELINE LAG BASELINE. Two contributions the driver can state -
        // the output lookahead ahead of the record and the converter's own
        // group delay - and it is a PREDICTION, not a measurement: on this
        // device family the derivation undershot 1449 measured frames by 1154.
        // It is what the FIRST play runs on, and the first
        // play (the sysid record, broadband by construction) is what replaces
        // it; see `measure_lag_xcorr`.
        //
        // Why it matters at all: TWR's sysID nfft is 2048, so an UNALIGNED
        // response puts drive and response in DIFFERENT FFT segments - they are
        // then unrelated inside every segment and the coherence estimate
        // collapses (0.010 in-band on a jumpered WIRE, where a wire is a
        // perfectly coherent plant).
        self.pipeline_lag_frames =
            self.lookahead_frames + actual.group_delay_frames.unwrap_or(0) as usize;
        self.lag_source = LagSource::Derived;

        // Bring-up verification, then PARK.
        self.backend.arm().map_err(hal_err)?;
        self.backend.stop().map_err(hal_err)?;

        // Size the input scratch ONCE so a play allocates nothing on that side.
        self.in_scratch = vec![0.0f32; self.block_frames * self.n_inputs];
        self.cancel.store(false, Ordering::Relaxed);
        self.configured = true;
        Ok(())
    }

    /// ONE play on its own cold task: arm -> commit what fits -> start ->
    /// stream the remainder -> capture -> stop (park + unreserve).
    ///
    /// The returned record is the SAME length as `drive` and aligned to it:
    /// the buffer carries an exact-zero pad past the record, and the response's
    /// first `pipeline_lag_frames` frames are dropped, so `response[k]` is the
    /// plant output while `drive[k]` was commanded. Everything downstream - the
    /// FRF, the coherence gate, the inverse update - assumes that pairing.
    fn play_cycle(&mut self, drive: &[f64]) -> Result<Option<Vec<f64>>, TwrError> {
        if !self.configured {
            return Err(TwrError::State("play() before arm()".into()));
        }
        if drive.is_empty() {
            return Err(TwrError::Play("the drive record is empty".into()));
        }
        let n_inputs = self.n_inputs;
        let n_drive = self.n_drive();
        let control_slot = self.control_slot;
        let pad = self.rest_pad_frames();
        let total = drive.len() + pad;

        // THE BUFFER: the record, then EXACT ZEROS. `f64 -> f32` of a literal
        // 0.0 is bit-exactly 0.0f32, which is what `stop()`'s zero-guard tests;
        // a taper of the record's tail would not be.
        self.drive_scratch.clear();
        self.drive_scratch.reserve(total * n_drive);
        for &d in drive {
            for _ in 0..n_drive {
                self.drive_scratch.push(d as f32);
            }
        }
        self.drive_scratch.resize(total * n_drive, 0.0f32);

        self.backend.arm().map_err(hal_err)?;
        let committed = self.commit_what_fits()?;
        self.backend.start().map_err(hal_err)?;

        let play = eigen_hal::BlockPlay::new(self.block_frames);
        let mut raw: Vec<f64> = Vec::with_capacity(total);
        // The tap and the observer are CLONED HANDLES, not moved fields: both
        // are shared with a host that may be reading them on another thread
        // while this play runs. The clones are two `Arc` bumps per play.
        let tap = self.tap.clone();
        let observer = self.play_observer.clone();
        let played = eigen_hal::play_block_wise(
            &mut self.backend,
            &self.drive_scratch,
            n_drive,
            n_inputs,
            &mut self.in_scratch,
            &play,
            committed,
            Some(&self.cancel),
            |block, frames| {
                let first = raw.len();
                for f in 0..frames {
                    raw.push(block[f * n_inputs + control_slot] as f64);
                }
                // ONE lock for the whole block. See `BackendReferencePlayer::tap`
                // for why per-sample locking is the thing being avoided; a
                // POISONED ring is skipped rather than panicking, because a
                // display tap must never be able to kill a run that is driving.
                if let Some(r) = tap.as_ref() {
                    if let Ok(mut ring) = r.lock() {
                        for y in &raw[first..] {
                            ring.push(*y);
                        }
                    }
                }
                if let Some(obs) = observer.as_ref() {
                    obs(PlayProgress {
                        frames_done: raw.len(),
                        frames_expected: total,
                    });
                }
            },
        );

        // PARK, on every path including cancel. A cancelled play must not leave
        // a started task behind it, and on the cancel path this call IS the
        // fade: the HAL appends its own raised-cosine taper behind whatever the
        // device still holds (`ramp_drive_to_zero`, 250 ms). There is no
        // player-layer fade because a player cannot get ahead of a committed
        // DAQmx queue - mid-record preemption is refused by the hardware
        // (-200289), so the honest stop arithmetic is
        // `(frames_committed - frames_done)/fs + the taper`.
        let stopped = self.backend.stop();
        match played {
            Err(StreamError::Cancelled {
                frames_done,
                frames_expected,
                ..
            }) => {
                stopped.map_err(hal_err)?;
                return Err(TwrError::Cancelled {
                    frames_done,
                    frames_expected,
                });
            }
            Err(e) => return Err(TwrError::Play(e.to_string())),
            Ok(_) => {}
        }
        stopped.map_err(hal_err)?;

        // THE LAG, measured off the FIRST record of the run - which is the
        // sysid excitation, broadband by construction and the only record whose
        // spectrum is guaranteed to support a correlation peak.
        if self.lag_source == LagSource::Derived {
            match measure_lag_xcorr(drive, &raw, pad) {
                Some((lag, _r)) => {
                    self.pipeline_lag_frames = lag;
                    self.lag_source = LagSource::Measured;
                }
                None => self.lag_source = LagSource::DerivedNoPeak,
            }
        }

        let lag = self.pipeline_lag_frames;
        if raw.len() < lag + drive.len() {
            return Err(TwrError::Play(format!(
                "aligned response is short: {} raw frames, lag {lag}, record {} - the \
                 pipeline delay exceeds the {pad}-frame rest pad, so the record's tail was \
                 never measured",
                raw.len(),
                drive.len()
            )));
        }
        let mut response = raw;
        response.drain(..lag);
        response.truncate(drive.len());
        Ok(Some(response))
    }
}

/// One drain of the display tap: the samples still in the ring, newest last,
/// and how many were DROPPED to make room since the previous drain.
///
/// The drop count is not diagnostics padding. A tap that silently discards is
/// indistinguishable from a tap that is keeping up, and a display fed by the
/// second while it is actually the first is a display that lies about the rate
/// its own data arrived at.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TapDrain {
    pub samples: Vec<f64>,
    pub dropped: u64,
}

/// A fixed-capacity DROP-OLDEST ring of control-channel samples.
///
/// Sized once and never resized, so a push from inside the play loop is two
/// integer updates and one store - no allocation, no syscall, nothing that can
/// block the thread that is feeding the device. That is the whole requirement:
/// the tap is allowed to lose data, it is not allowed to cost time.
/// Drop-OLDEST rather than drop-newest because a display wants the most recent
/// window, not the first one it happened to catch.
///
/// PUBLIC since the daemon host landed, together with
/// [`BackendReferencePlayer::display_tap_handle`]: the daemon drains this ring from
/// a thread that is not the one playing, so the type has to be nameable outside this
/// crate. The mutex around it is the caller's (the handle is an
/// `Arc<Mutex<SampleRing>>`), which keeps the per-block locking decision at the one
/// place that knows the block rate.
pub struct SampleRing {
    buf: Vec<f64>,
    /// Index of the oldest live sample.
    head: usize,
    len: usize,
    dropped: u64,
}

impl SampleRing {
    /// A ring holding `capacity` samples (minimum 1).
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        SampleRing {
            buf: vec![0.0; cap],
            head: 0,
            len: 0,
            dropped: 0,
        }
    }

    /// Append one sample, dropping the oldest when the ring is full.
    #[inline]
    pub fn push(&mut self, v: f64) {
        let cap = self.buf.len();
        if self.len == cap {
            self.buf[self.head] = v;
            self.head = (self.head + 1) % cap;
            self.dropped += 1;
        } else {
            let at = (self.head + self.len) % cap;
            self.buf[at] = v;
            self.len += 1;
        }
    }

    /// Take everything held, oldest first, and the drop count since the last
    /// drain. Leaves the ring empty and its drop counter at zero.
    pub fn drain(&mut self) -> TapDrain {
        let cap = self.buf.len();
        let samples = (0..self.len)
            .map(|i| self.buf[(self.head + i) % cap])
            .collect();
        let dropped = self.dropped;
        self.head = 0;
        self.len = 0;
        self.dropped = 0;
        TapDrain { samples, dropped }
    }
}

/// The `|r|` a pure-noise pair reaches by chance over `n` samples.
///
/// The normalized correlation of two independent zero-mean sequences is
/// approximately `N(0, 1/n)`, so `4.1/sqrt(n)` is the ~4.1-sigma tail - the
/// level a search over a few thousand lags would otherwise cross routinely on
/// noise alone. It is the number that refuted the borrowed-estimator trap, and
/// it is stated here rather than tuned because a floor picked to
/// make a particular run pass is not a floor.
fn xcorr_noise_floor(n: usize) -> f64 {
    if n == 0 {
        return 1.0;
    }
    4.1 / (n as f64).sqrt()
}

/// Longest correlation window, frames. The search costs `window * (max_lag+1)`
/// multiply-adds and runs ONCE per run, so this bounds it at ~50 M for the
/// chassis's 3072-frame pad - tens of milliseconds against a multi-second
/// sysid record.
const XCORR_WINDOW_CAP: usize = 1 << 14;
/// Below this the floor is too loose to mean anything (`4.1/sqrt(64)` is 0.51).
const XCORR_MIN_WINDOW: usize = 64;

/// Measure the pipeline lag: the NON-NEGATIVE frame offset `lag` at which
/// `raw[lag..]` best correlates with `drive`, or `None` when no peak clears the
/// noise floor.
///
/// Mean-removed and energy-normalized on both sides, so `r` is comparable
/// across levels and a DC offset on the input cannot dominate every lag equally
/// and flatten the peak. Only non-negative lags are searched: a response cannot
/// precede the drive that caused it, and offering negative lags would let noise
/// win in a region physics has already excluded.
///
/// # Premises, checked against BOTH of TWR's generators (the borrowed-estimator rule)
///
/// The estimator needs an excitation whose autocorrelation is sharp at zero.
/// TWR's sysid record is either a COLA-flat band-limited random (broadband,
/// near-impulsive autocorrelation) or a LOG CHIRP (autocorrelation with
/// structure, but a single dominant peak). Both are exercised by
/// `tests/measured_lag.rs`; the chirp is the one that would break a borrowed
/// noise-only assumption, which is why it has its own test rather than a
/// comment claiming it works.
fn measure_lag_xcorr(drive: &[f64], raw: &[f64], max_lag: usize) -> Option<(usize, f64)> {
    if drive.is_empty() || raw.len() <= max_lag {
        return None;
    }
    let w = drive.len().min(raw.len() - max_lag).min(XCORR_WINDOW_CAP);
    if w < XCORR_MIN_WINDOW {
        return None;
    }
    let d = &drive[..w];
    let wf = w as f64;
    let sd: f64 = d.iter().sum();
    let sdd: f64 = d.iter().map(|x| x * x).sum();
    let var_d = sdd - sd * sd / wf;
    if var_d <= 0.0 {
        return None;
    }
    // Prefix sums over the response so each candidate lag's window mean and
    // energy are O(1) and only the dot product is O(w).
    let need = w + max_lag;
    let mut ps = vec![0.0f64; need + 1];
    let mut pss = vec![0.0f64; need + 1];
    for i in 0..need {
        ps[i + 1] = ps[i] + raw[i];
        pss[i + 1] = pss[i] + raw[i] * raw[i];
    }
    let mut best_lag = 0usize;
    let mut best_r = 0.0f64;
    for lag in 0..=max_lag {
        let sy = ps[lag + w] - ps[lag];
        let syy = pss[lag + w] - pss[lag];
        let var_y = syy - sy * sy / wf;
        if var_y <= 0.0 {
            continue;
        }
        let dot: f64 = d
            .iter()
            .zip(raw[lag..lag + w].iter())
            .map(|(a, b)| a * b)
            .sum();
        let r = (dot - sd * sy / wf) / (var_d.sqrt() * var_y.sqrt());
        if r.abs() > best_r.abs() {
            best_r = r;
            best_lag = lag;
        }
    }
    if best_r.abs() >= xcorr_noise_floor(w) {
        Some((best_lag, best_r))
    } else {
        None
    }
}

impl<B: Backend> ReferencePlayer for BackendReferencePlayer<B> {
    fn path(&self) -> TwrPath {
        TwrPath::HostBackend
    }

    fn fs_hz(&self) -> f64 {
        self.backend.caps().sample_rate_hz
    }

    fn lag_provenance(&self) -> Option<LagSource> {
        Some(self.lag_source)
    }

    fn lag_frames(&self) -> Option<usize> {
        Some(self.pipeline_lag_frames)
    }

    fn committed_frames(&self) -> Option<usize> {
        Some(self.last_committed)
    }

    fn take_driver_warnings(&mut self) -> Vec<String> {
        self.backend.take_warnings()
    }

    fn arm(&mut self) -> Result<(), TwrError> {
        self.open_and_configure()
    }

    fn actual_fs_hz(&self) -> Option<f64> {
        self.actual_fs_hz
    }

    fn set_reference_rate_hz(&mut self, hz: f64) {
        self.reference_rate_hz = Some(hz);
    }

    fn station_label(&self) -> Option<String> {
        Some(self.station.to_string())
    }

    fn drain_display_tap(&mut self) -> Option<TapDrain> {
        // A POISONED ring answers `None` rather than panicking: the tap is a
        // display convenience and must never be able to take a run down.
        self.tap
            .as_ref()
            .and_then(|r| r.lock().ok().map(|mut g| g.drain()))
    }

    fn play(&mut self, drive: &[f64]) -> Result<Option<Vec<f64>>, TwrError> {
        self.play_cycle(drive)
    }

    fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn stop(&mut self) -> Result<(), TwrError> {
        self.configured = false;
        self.backend.stop().map_err(hal_err)
    }

    fn emergency_stop(&mut self) -> Result<(), TwrError> {
        self.configured = false;
        self.backend.emergency_stop().map_err(hal_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gain-2 loopback test double.
    ///
    /// Beware the test double that encodes the bug. This used to hold ONE
    /// `pending` sample and fill every slot of `out` with it while returning
    /// `Frames(1)` regardless of the buffer size - i.e. it modelled the retired
    /// `SimBackend` drive LATCH, not a device. Under block-wise play that answers
    /// one frame of the block's LAST sample per read. Verdict: the DOUBLE was
    /// wrong, the alignment ASSERTION it served was right, so the double is
    /// rewritten to buffer per frame (what the Sim now really does) and the
    /// assertion stands unchanged.
    #[derive(Default)]
    struct Loopback {
        armed: bool,
        /// Buffered drive samples, consumed one per generated frame.
        buffered: std::collections::VecDeque<f32>,
        /// Drive FRAMES accepted (not `write_drive` CALLS - the call count is a
        /// property of the block size, and pinning it would re-encode the
        /// one-sample-per-call shape this change removed).
        frames_accepted: usize,
        /// Every sample handed to `write_drive`, in order. The pad assertion
        /// needs the VALUES, not just the count.
        written: Vec<f32>,
        /// Lifecycle counters: the per-play cycle is a claim about which of
        /// these move per play and which do not.
        opens: usize,
        configures: usize,
        starts: usize,
        stops: usize,
    }

    impl Backend for Loopback {
        fn caps(&self) -> eigen_hal::Caps {
            eigen_hal::Caps {
                loop_owner: eigen_hal::LoopOwner::HostClosesLoop,
                native_modes: vec![],
                clock: eigen_hal::ClockDomain::Shared,
                max_input_channels: 2,
                max_output_channels: 1,
                abort_latency_bound: None,
                output_buffer: None,
                sample_rate_hz: 1000.0,
                // A host-loop test double delegates nothing and has one implicit
                // clock group.
                delegated_response_observable: false,
                clock_groups: vec![],
            }
        }
        fn open(&mut self) -> Result<(), HalError> {
            self.opens += 1;
            Ok(())
        }
        fn close(&mut self) -> Result<(), HalError> {
            Ok(())
        }
        fn configure(
            &mut self,
            _t: &ChannelTable,
            acq: &eigen_hal::AcquisitionConfig,
        ) -> Result<eigen_hal::ConfiguredActual, HalError> {
            self.configures += 1;
            Ok(eigen_hal::ConfiguredActual {
                actual_rate_hz: acq.requested_rate_hz,
                block_frames: acq.block_frames,
                group_delay_frames: None,
            })
        }
        fn arm(&mut self) -> Result<(), HalError> {
            self.armed = true;
            Ok(())
        }
        fn start(&mut self) -> Result<(), HalError> {
            self.starts += 1;
            Ok(())
        }
        fn stop(&mut self) -> Result<(), HalError> {
            self.armed = false;
            self.stops += 1;
            Ok(())
        }
        fn read_input(&mut self, out: &mut [f32]) -> Result<eigen_hal::InputRead, HalError> {
            // One buffered drive sample per generated frame, gain of 2.
            let n = out.len().min(self.buffered.len());
            if n == 0 {
                return Ok(eigen_hal::InputRead::NoDataYet);
            }
            for slot in out.iter_mut().take(n) {
                *slot = self.buffered.pop_front().unwrap_or(0.0) * 2.0;
            }
            Ok(eigen_hal::InputRead::Frames(n))
        }
        fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
            self.buffered.extend(drive.iter().copied());
            self.written.extend_from_slice(drive);
            self.frames_accepted += drive.len();
            // 1 drive channel: every sample is a frame, all accepted.
            Ok(drive.len())
        }
        fn poll_status(&mut self) -> eigen_hal::BackendStatus {
            eigen_hal::BackendStatus::default()
        }
        fn emergency_stop(&mut self) -> Result<(), HalError> {
            Ok(())
        }
    }

    fn table() -> ChannelTable {
        let mut t = ChannelTable::new();
        let mut c = eigen_channel_table::Channel::new(1);
        c.role = ChannelRole::Control;
        t.channels.push(c);
        let mut d = eigen_channel_table::Channel::new(2);
        d.role = ChannelRole::Drive;
        t.channels.push(d);
        t
    }

    #[test]
    fn play_before_arm_is_a_state_error() {
        let mut p = BackendReferencePlayer::new(
            Loopback::default(),
            table(),
            eigen_module_abi::StationClass::EigenSim,
        )
        .unwrap();
        assert!(matches!(p.play(&[0.0]), Err(TwrError::State(_))));
    }

    #[test]
    fn play_returns_an_aligned_response_and_a_zero_padded_buffer() {
        let mut p = BackendReferencePlayer::new(
            Loopback::default(),
            table(),
            eigen_module_abi::StationClass::EigenSim,
        )
        .unwrap();
        p.arm().unwrap();
        let drive = vec![0.25, -0.5, 1.0];
        let resp = p.play(&drive).unwrap().unwrap();
        assert_eq!(resp.len(), 3);
        // Gain-2 loopback, aligned sample-for-sample.
        assert!((resp[0] - 0.5).abs() < 1e-6);
        assert!((resp[1] + 1.0).abs() < 1e-6);
        assert!((resp[2] - 2.0).abs() < 1e-6);
        // Record + rest pad, counted in FRAMES. No flush head: the pre-play
        // flush is gone and the quiet window rides at the END of the play.
        let pad = p.rest_pad_frames();
        // No output contract and a 64-frame block, so the ring-down window is
        // what sets the pad here.
        assert_eq!(pad, DEFAULT_QUIET_FRAMES);
        assert_eq!(p.backend().frames_accepted, drive.len() + pad);
        // A 3-sample record cannot support a correlation window, so the
        // measurement declines and SAYS SO rather than inventing a lag.
        assert_eq!(p.lag_source(), LagSource::DerivedNoPeak);
        assert_eq!(p.path(), TwrPath::HostBackend);
        assert_eq!(p.fs_hz(), 1000.0);
    }

    /// The last thing the device is handed is bit-exactly `0.0f32`, which is
    /// what `NiBackend::stop()`'s zero-guard tests before deciding whether to
    /// append its 250 ms taper. A taper of the record's tail would
    /// end near zero and fail this.
    #[test]
    fn the_played_buffer_ends_in_exact_zeros() {
        let mut p = BackendReferencePlayer::new(
            Loopback::default(),
            table(),
            eigen_module_abi::StationClass::EigenSim,
        )
        .unwrap();
        p.arm().unwrap();
        p.play(&[0.7, -0.7, 0.7]).unwrap();
        let pad = p.rest_pad_frames();
        let written = &p.backend().written;
        assert_eq!(written.len(), 3 + pad);
        for (i, &v) in written.iter().skip(3).enumerate() {
            assert!(
                v == 0.0f32 && v.to_bits() == 0u32,
                "pad frame {i} is {v} (bits {:#x}); the guard tests bit-exact 0.0",
                v.to_bits()
            );
        }
    }

    /// The whole point of the split: a run configures ONCE, and
    /// the per-play cycle is arm/start/stop only. Configuring per play would
    /// re-derive `block_frames` and the lag on every pass.
    #[test]
    fn configure_runs_once_across_many_plays_and_the_lag_never_moves() {
        let mut p = BackendReferencePlayer::new(
            Loopback::default(),
            table(),
            eigen_module_abi::StationClass::EigenSim,
        )
        .unwrap();
        p.arm().unwrap();
        let lag = p.pipeline_lag_frames();
        for _ in 0..5 {
            p.play(&[0.1, 0.2, 0.3]).unwrap();
            assert_eq!(p.pipeline_lag_frames(), lag, "the lag moved mid-run");
        }
        assert_eq!(p.backend().configures, 1, "configure() per play");
        assert_eq!(p.backend().opens, 1, "open() per play");
        assert_eq!(p.backend().starts, 5, "one start per play");
        assert_eq!(
            p.backend().stops,
            6,
            "one park at open_and_configure plus one per play"
        );
    }

    /// A raised cancel flag ends the play as `Cancelled`, not as a generic play
    /// error, and the task is parked on the way out.
    #[test]
    fn a_raised_cancel_flag_ends_the_play_cancelled_and_parks() {
        let mut p = BackendReferencePlayer::new(
            Loopback::default(),
            table(),
            eigen_module_abi::StationClass::EigenSim,
        )
        .unwrap();
        p.arm().unwrap();
        p.request_cancel();
        let err = p.play(&[0.1; 512]).unwrap_err();
        match err {
            TwrError::Cancelled {
                frames_done,
                frames_expected,
            } => {
                assert_eq!(frames_done, 0, "cancelled before the first block");
                assert_eq!(frames_expected, 512 + p.rest_pad_frames());
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
        assert!(!p.backend().armed, "the cancel path must park the task");
    }

    /// A WIDE table is refused at bring-up, by name and with the count.
    ///
    /// The alternative is what shipped: `n_drive` hard-coded to 1 at both write
    /// sites, so a 2-drive table makes `play_block_wise` read the mono record
    /// as interleaved pairs and the record alternates across two DACs at half
    /// rate. That does not read as an error anywhere downstream - coherence
    /// stays high and |H| comes back 2x wrong - which is exactly why it must
    /// not stay reachable.
    ///
    /// RED-MUTATION: restore `fn n_drive(&self) -> usize { 1 }` and this test
    /// reds (the wide table arms and plays silently).
    #[test]
    fn a_two_drive_table_is_refused_by_name_at_bring_up() {
        let mut t = table();
        let mut d2 = eigen_channel_table::Channel::new(3);
        d2.role = ChannelRole::Drive;
        t.channels.push(d2);
        let mut p = BackendReferencePlayer::new(
            Loopback::default(),
            t,
            eigen_module_abi::StationClass::EigenSim,
        )
        .expect("a wide table is VALID; the refusal is TWR's, not the table's");
        let text = p.arm().expect_err("two drive rows must refuse").to_string();
        assert!(
            text.contains('2'),
            "the refusal must name the count: {text}"
        );
        assert!(text.contains("Drive rows"), "{text}");
        assert!(
            text.contains("half") && text.contains("coherence"),
            "the refusal must say what the silent failure would look like: {text}"
        );
        assert_eq!(
            p.backend().starts,
            0,
            "nothing may be clocked out on a table this player cannot honor"
        );
    }

    /// The other half: a table with no Drive row cannot play anything, and says
    /// so at construction rather than at the first write.
    #[test]
    fn a_table_with_no_drive_row_is_refused_at_construction() {
        let mut t = ChannelTable::new();
        let mut c = eigen_channel_table::Channel::new(1);
        c.role = ChannelRole::Control;
        t.channels.push(c);
        let built = BackendReferencePlayer::new(
            Loopback::default(),
            t,
            eigen_module_abi::StationClass::EigenSim,
        );
        let err = match built {
            Err(e) => e,
            Ok(_) => panic!("a table with no Drive row must not build a record player"),
        };
        assert!(err.to_string().contains("no enabled Drive channel"));
    }

    /// The display tap is bounded, drop-OLDEST, and COUNTS what it dropped.
    ///
    /// A tap that silently discards is indistinguishable from one that is
    /// keeping up, so the count is part of the contract, not diagnostics.
    #[test]
    fn the_display_tap_is_bounded_drop_oldest_and_counts_drops() {
        let mut p = BackendReferencePlayer::new(
            Loopback::default(),
            table(),
            eigen_module_abi::StationClass::EigenSim,
        )
        .unwrap()
        .with_display_tap(8);
        p.arm().unwrap();

        let drive: Vec<f64> = (1..=20).map(|i| i as f64).collect();
        p.play(&drive).unwrap();

        let pad = p.rest_pad_frames();
        let drained = p.drain_display_tap().expect("a tap was attached");
        assert_eq!(drained.samples.len(), 8, "the ring is bounded");
        // Every INPUT frame is tapped, record and pad alike - the tap sits on
        // the input path, not on the record. The pad's response is the tail, so
        // the newest 8 are pad zeros against this loopback.
        assert_eq!(
            drained.dropped,
            (drive.len() + pad - 8) as u64,
            "the drop count must account for every frame the ring could not keep"
        );
        for (i, v) in drained.samples.iter().enumerate() {
            assert_eq!(*v, 0.0, "tail sample {i} should be pad silence: {v}");
        }
        // Draining empties it: the next drain reports nothing, not the same
        // window again.
        let again = p.drain_display_tap().unwrap();
        assert!(again.samples.is_empty() && again.dropped == 0);
    }

    /// A tap the caller keeps up with loses nothing, which is the other half of
    /// the claim - a ring that always reports drops proves only that it is too
    /// small.
    #[test]
    fn a_drained_display_tap_loses_nothing() {
        let mut p = BackendReferencePlayer::new(
            Loopback::default(),
            table(),
            eigen_module_abi::StationClass::EigenSim,
        )
        .unwrap()
        .with_display_tap(4096);
        p.arm().unwrap();
        let drive = vec![0.5f64; 16];
        p.play(&drive).unwrap();
        let d = p.drain_display_tap().unwrap();
        assert_eq!(d.dropped, 0);
        assert_eq!(d.samples.len(), drive.len() + p.rest_pad_frames());
        for (i, v) in d.samples.iter().take(drive.len()).enumerate() {
            assert!((v - 1.0).abs() < 1e-6, "tapped sample {i} = {v}");
        }
    }

    /// **THE HOST READ PATH: the tap is drainable through a SHARED handle while
    /// the player is owned elsewhere.**
    ///
    /// This is the property the daemon host is built on. Its pump holds the
    /// service (and therefore the player) for the whole of a play, so the ONLY
    /// way a 30 s record can feed a live time pane is a handle taken before the
    /// player was moved. The test drains through the handle and never touches
    /// the player, which is exactly what the host's drain thread does.
    ///
    /// RED-MUTATION: make `display_tap_handle` return `None` and the drain
    /// finds no ring; revert `tap` to an unshared field and this does not
    /// compile, which is the stronger half.
    #[test]
    fn the_display_tap_drains_through_a_shared_handle_the_player_does_not_own() {
        let mut p = BackendReferencePlayer::new(
            Loopback::default(),
            table(),
            eigen_module_abi::StationClass::EigenSim,
        )
        .unwrap()
        .with_display_tap(4096);
        let handle = p.display_tap_handle().expect("a tap was attached");
        p.arm().unwrap();
        let drive = vec![0.5f64; 16];
        p.play(&drive).unwrap();

        // Drained WITHOUT the player: `handle` is all a second thread has.
        let d = handle.lock().unwrap().drain();
        assert_eq!(d.dropped, 0);
        assert_eq!(d.samples.len(), drive.len() + p.rest_pad_frames());
        for (i, v) in d.samples.iter().take(drive.len()).enumerate() {
            assert!((v - 1.0).abs() < 1e-6, "tapped sample {i} = {v}");
        }
        // ...and the player's own drain then finds it EMPTY, which is what
        // proves the two views are one ring rather than two.
        assert!(p.drain_display_tap().unwrap().samples.is_empty());
    }

    /// **THE PER-BLOCK PROGRESS OBSERVER** reports monotone `frames_done`
    /// against a constant `frames_expected`, and reaches the total.
    ///
    /// The daemon's numeric status through a long play is exactly this
    /// sequence. Monotonicity is asserted rather than assumed
    /// because a status that goes backwards reads to an operator as a restarted
    /// run.
    ///
    /// RED-MUTATION: report `frames` (the block length) instead of `raw.len()`
    /// and the monotone assertion reds on the second block.
    #[test]
    fn the_play_observer_reports_monotone_progress_to_the_expected_total() {
        let seen: Arc<Mutex<Vec<PlayProgress>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let mut p = BackendReferencePlayer::new(
            Loopback::default(),
            table(),
            eigen_module_abi::StationClass::EigenSim,
        )
        .unwrap()
        .with_block_frames(4)
        .with_play_observer(Arc::new(move |pr| {
            sink.lock().expect("sink poisoned").push(pr);
        }));
        p.arm().unwrap();
        let drive = vec![0.25f64; 40];
        p.play(&drive).unwrap();

        let rows = seen.lock().unwrap().clone();
        assert!(
            rows.len() > 1,
            "a multi-block play must report more than once: {rows:?}"
        );
        let expected = drive.len() + p.rest_pad_frames();
        let mut last = 0usize;
        for r in &rows {
            assert_eq!(
                r.frames_expected, expected,
                "the total must not move: {r:?}"
            );
            assert!(
                r.frames_done > last,
                "progress must ADVANCE: {last} -> {r:?}"
            );
            last = r.frames_done;
        }
        assert_eq!(last, expected, "the last report is the whole play");
    }

    /// Off by default: a tap nobody asked for is a ring nobody drains.
    #[test]
    fn there_is_no_display_tap_unless_one_is_asked_for() {
        let mut p = BackendReferencePlayer::new(
            Loopback::default(),
            table(),
            eigen_module_abi::StationClass::EigenSim,
        )
        .unwrap();
        p.arm().unwrap();
        p.play(&[0.1, 0.2]).unwrap();
        assert!(p.drain_display_tap().is_none());
    }

    /// The honesty label's station comes from the station, not from a constant.
    #[test]
    fn the_station_label_is_the_station() {
        let p = BackendReferencePlayer::new(
            Loopback::default(),
            table(),
            eigen_module_abi::StationClass::NiReal,
        )
        .unwrap();
        assert_eq!(p.station_label().as_deref(), Some("NI-REAL"));
    }

    #[test]
    fn capability_table_is_the_single_source() {
        let host = TwrPathCapabilities::for_path(TwrPath::HostBackend);
        assert!(host.adaptation_runs_hostside && host.response_observable);
        let delegated = TwrPathCapabilities::for_path(TwrPath::Delegated);
        // The TWR twist: adaptation is host-side EVEN delegated...
        assert!(delegated.adaptation_runs_hostside);
        // ...but the loop cannot close until the response is observable.
        assert!(!delegated.response_observable);
    }
}
