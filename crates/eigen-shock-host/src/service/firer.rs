//! The fire-path seam the classical-shock service drives: one trait,
//! two honest implementations - the Path-B host firer over any `eigen_hal`
//! [`Backend`] (the Sim plant in tests; the real NI chassis today) and the
//! Path-A delegated firer that lives with the delegated backend (the
//! uncompensated-download + absolute-dB seam, tested over a mock transport).
//!
//! # This file is the hal-NEUTRAL half
//!
//! The trait, [`FireControl`], [`quiet_lead_frames`] and [`BackendPulseFirer`]
//! name no concrete backend, which is what lets them live in this crate at all.
//! `DelegatedPulseFirer` - the half that imports the private delegated backend -
//! is the reason the Path-A crate still exists separately; it sits in that
//! crate's `service::delegated_firer` and is re-exported from its
//! `service::firer`, so every pre-extraction path still resolves.
//!
//! # Why a trait and not two services
//!
//! The per-pulse pipeline (record -> tolerance -> limits -> ladder) is
//! path-independent; only HOW a pulse fires and WHETHER its response is
//! observable differ. The trait pins exactly that split:
//!
//! - [`PulseFirer::fire`] takes the ABSOLUTE commanded waveform (Path B plays
//!   it; the delegated path ignores it - its reference was downloaded at arm)
//!   plus the absolute commanded dB (the delegated setpoint;
//!   redundant on Path B).
//! - The return is `Option<Vec<f64>>`: `Some(record)` where the response is
//!   measurable (Path B), `None` where it is not (delegated - inbound telemetry
//!   is undecoded). The service records honesty accordingly instead
//!   of faking a measurement.
//!
//! # The rate is the DEVICE's
//!
//! [`PulseFirer::actual_fs_hz`] is the adoption seam. `ClassicalTestConfig::fs_hz`
//! is a REQUEST; what the device actually programmed comes back through
//! [`eigen_hal::ConfiguredActual`] and is what every Hz-denominated thing the
//! service owns must be built at. The firer stores it at arm and the service
//! reads it back BEFORE it synthesizes anything (see
//! `ClassicalShockService::arm_inner`). Before the 2026-08-08 rework the service
//! synthesized at 5120 Hz and the chassis played the record at 51 200 Hz, so an
//! 11 ms pulse went out ~1.1 ms wide and every downstream number described a
//! waveform that never existed.
//!
//! # Per-shot cold task
//!
//! [`BackendPulseFirer::arm`] verifies bring-up and then PARKS both tasks; no
//! task runs between arm and fire. Each [`BackendPulseFirer::fire`] is its own
//! cold arm -> commit -> start -> play -> stop cycle. See `fire`'s body for the
//! invariant that buys.

use crate::service::ServiceError;
use eigen_channel_table::{ChannelRole, ChannelTable};
use eigen_hal::{Backend, HalError, StreamError};
use eigen_shock_common::DrivePath;
use std::sync::atomic::AtomicBool;

/// Quiet surround as a fraction of the MAIN pulse duration (see
/// [`quiet_lead_frames`]).
pub const QUIET_LEAD_FRACTION_OF_PULSE: f64 = 0.5;

/// Absolute floor on the quiet surround, seconds (see [`quiet_lead_frames`]).
pub const MIN_QUIET_LEAD_S: f64 = 0.005;

/// The quiet lead-in / lead-out each Path-B pulse is surrounded with, in FRAMES
/// at the rate the device is ACTUALLY running.
///
/// TIME-denominated, not sample-denominated. The retired
/// `DEFAULT_QUIET_SAMPLES = 200` meant 39 ms at the 5120 Hz records used to be
/// synthesized at and 3.9 ms at the 51 200 Hz this chassis really runs - one
/// constant standing for two different physical durations, and at the higher rate
/// too short for MIL-STD-810's `+/-0.4 T` quiet surround window on an 11 ms pulse
/// (4.4 ms each side). A tolerance window clipped by the record edge is not a
/// failure the checker reports; it silently checks FEWER samples
/// (`EnvelopeCheck::checked_samples`), which is a green earned by not looking.
///
/// The rule is `max(0.5 x T_pulse, 5 ms)`: half the pulse duration clears the
/// 0.4 T window with margin on both sides whatever the rate, and the 5 ms floor
/// keeps a very short pulse from being surrounded by almost nothing.
pub fn quiet_lead_frames(pulse_duration_s: f64, fs_hz: f64) -> usize {
    if !pulse_duration_s.is_finite() || !fs_hz.is_finite() || fs_hz <= 0.0 {
        return 0;
    }
    let seconds = (QUIET_LEAD_FRACTION_OF_PULSE * pulse_duration_s.max(0.0)).max(MIN_QUIET_LEAD_S);
    let frames = (seconds * fs_hz).ceil();
    if frames.is_finite() && frames >= 0.0 {
        frames as usize
    } else {
        0
    }
}

/// Live per-fire channels from the SERVICE into the fire path.
///
/// Grouped into one borrowed struct rather than added as two more positional
/// arguments, because both are host-owned taps that will grow (the daemon's
/// StatusCell attaches to `observer`) and a fire signature that grows a parameter
/// per tap is a signature nobody can read.
pub struct FireControl<'a> {
    /// Raised by the HOST to cancel the play at the next block boundary.
    ///
    /// **TRUE means STOP** - the opposite sense to the service's train-loop
    /// `should_continue` flag, which is TRUE to keep going. The two meet in this
    /// call path (`ClassicalShockService::fire_train_cancellable`) and are never
    /// interchangeable.
    ///
    /// What the device has already ACCEPTED keeps playing: mid-record preemption
    /// is refused by this chassis (`-200289`). See
    /// [`eigen_hal::StreamError::Cancelled`] and `ClassicalShockService::abort`.
    pub cancel: Option<&'a AtomicBool>,
    /// Called once per DELIVERED input block with
    /// `(frames_delivered_so_far, frames_expected)`, from INSIDE the play loop.
    ///
    /// The per-block observer seam: a watchdog beat, a StatusCell write and the
    /// display tap all need to advance DURING a fire, not after it, and a fire is
    /// the longest thing the service does without returning.
    ///
    /// `+ Send + 'static` matches what the service OWNS
    /// (`Box<dyn FnMut(usize, usize) + Send>`), and it has to be spelled out
    /// rather than elided: behind `&'a mut` a trait object is INVARIANT, so an
    /// elided object-lifetime bound of `'a` would force `'a = 'static` at the
    /// call site and the borrow could never be released.
    pub observer: Option<&'a mut (dyn FnMut(usize, usize) + Send + 'static)>,
}

impl<'a> FireControl<'a> {
    /// No cancel channel and no observer - the plain synchronous fire.
    pub fn none() -> Self {
        FireControl {
            cancel: None,
            observer: None,
        }
    }

    /// Just a cancel flag.
    pub fn with_cancel(cancel: Option<&'a AtomicBool>) -> Self {
        FireControl {
            cancel,
            observer: None,
        }
    }
}

impl Default for FireControl<'_> {
    fn default() -> Self {
        FireControl::none()
    }
}

/// The fire-path seam (see the module docs).
pub trait PulseFirer {
    /// Which drive path this firer drives - the ONE capability discriminant
    /// (the service greys features off
    /// `DrivePath::capabilities`).
    fn path(&self) -> DrivePath;

    /// Open the fire path (Path B: open/configure/gate/verify, then PARK both
    /// tasks; delegated: configure + overlay + UNCOMPENSATED reference
    /// download + arm). Called once by the service's arm step.
    fn arm(&mut self) -> Result<(), ServiceError>;

    /// The rate the fire path is ACTUALLY running at, Hz - the adoption seam
    /// (the device owns the rate). `None` before [`PulseFirer::arm`] has succeeded,
    /// because until the device has been configured the rate is a REQUEST, not a
    /// fact.
    ///
    /// The service REFUSES to arm on `None` after a successful `arm()`: a firer
    /// that cannot say what rate it runs at cannot have anything denominated
    /// against it, and defaulting to the requested rate is precisely the
    /// requested-rate defect this seam exists to prevent.
    fn actual_fs_hz(&self) -> Option<f64>;

    /// Set the quiet lead-in / lead-out surrounding each pulse, in FRAMES at the
    /// adopted rate. The service computes it from the pulse duration through
    /// [`quiet_lead_frames`] once the rate is known; paths that do not play a
    /// host-side record ignore it.
    fn set_quiet_frames(&mut self, _frames: usize) {}

    /// Fire ONE pulse.
    ///
    /// - `drive_g`: the absolute commanded waveform, g (level + drive-limit
    ///   scaling already applied), synthesized at the ADOPTED rate. Path B plays
    ///   it into the DAC; the delegated path IGNORES it (the device holds the
    ///   downloaded reference and scales by the setpoint).
    /// - `commanded_db`: the absolute commanded level in dB (the delegated
    ///   `10^(dB/20)` setpoint).
    /// - `ctl`: the host's live channels - see [`FireControl`].
    ///
    /// Returns `Some(measured)` - the control-channel acceleration record at
    /// the fired level - where the response is observable, `None` where it is
    /// not (delegated path). A host cancel returns
    /// [`ServiceError::Cancelled`] carrying the committed-queue arithmetic.
    fn fire(
        &mut self,
        drive_g: &[f64],
        commanded_db: f64,
        ctl: &mut FireControl<'_>,
    ) -> Result<Option<Vec<f64>>, ServiceError>;

    /// Take any non-fatal DRIVER diagnostics accumulated since the last call
    /// (DAQmx underruns, rate coercions, late samples), draining them.
    ///
    /// A passthrough to [`Backend::take_warnings`], which no shock caller used to
    /// drain - the NI backend has always collected these and the whole shock path
    /// was structurally unable to see them. The default
    /// is empty so a path with no driver beneath it costs nothing.
    fn take_warnings(&mut self) -> Vec<String> {
        Vec::new()
    }

    /// Close the fire path cleanly: park the tasks, unreserve, drive off
    /// (backend stop / delegated shock_stop). Idempotent.
    fn stop(&mut self) -> Result<(), ServiceError>;

    /// EMERGENCY stop: the abort verb. Ramp the drive down and kill the tasks
    /// through [`Backend::emergency_stop`], which also unreserves them (so
    /// abort -> re-arm -> fire is not a zombie).
    ///
    /// NOT immediate, and the service's `abort` doc carries the arithmetic:
    /// the raised-cosine ramp plus the committed drive queue bound how long
    /// drive stays on the wire, because mid-record preemption is REFUSED by this
    /// chassis (`-200289`). It also stops the INPUT task, so abort evidence can
    /// never be AI-side.
    fn emergency_stop(&mut self) -> Result<(), ServiceError>;
}

fn hal_err(e: HalError) -> ServiceError {
    ServiceError::Fire(e.to_string())
}

/// Map a block-play failure onto the service's error type, keeping CANCELLED
/// typed (the service has to tell an operator Stop apart from a fault).
fn stream_err(e: StreamError) -> ServiceError {
    match e {
        StreamError::Cancelled {
            frames_done,
            frames_committed,
            frames_expected,
        } => ServiceError::Cancelled {
            frames_done,
            frames_committed,
            frames_expected,
        },
        other => ServiceError::Fire(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Path B: the host firer over any eigen-hal Backend
// ---------------------------------------------------------------------------

/// The Path-B (HostDac) firer: plays the commanded waveform BLOCK-WISE into any
/// [`Backend`] and reads back the control channel. In tests the backend is the
/// eigen-hal-sim plant; on the bench it is the NI chassis.
///
/// It used to step ONE SAMPLE PER CALL, the shape the retired `SimBackend` drive
/// latch forced. Delivery now goes through [`eigen_hal::play_block_wise`]: a write
/// honors `write_drive`'s ACCEPTED count and re-offers the remainder, a
/// `NoDataYet` read is POLLED against a deadline rather than being an instant
/// fatal, and only frames the backend actually delivered are recorded. `Ended`
/// mid-fire stays fatal.
pub struct BackendPulseFirer<B: Backend> {
    /// The station class the arm gate is given. Supplied
    /// by whoever built the backend, because the class is a property of what the
    /// backend is bound to, not of `B`.
    station: eigen_module_abi::StationClass,
    backend: B,
    table: ChannelTable,
    /// Interleaved input frame width: enabled non-Drive channels, in table
    /// order (the backend's configure mapping).
    n_inputs: usize,
    /// The slot (within the input frame) of the first enabled Control channel.
    control_slot: usize,
    /// Quiet frames driven before and after each pulse, at the ADOPTED rate.
    /// Set by the service at arm through [`PulseFirer::set_quiet_frames`].
    quiet_frames: usize,
    /// Frames per write/read block, from what the backend CONFIGURED. Set in
    /// `arm()`; 1 until then.
    block_frames: usize,
    /// The block this firer REQUESTS at configure. `None` = derive it from the
    /// rate (`eigen_hal::default_block_frames`).
    requested_block: Option<usize>,
    /// The rate the device ACTUALLY programmed, from `ConfiguredActual`. The
    /// adoption seam; `None` until `arm()` succeeds.
    actual_fs_hz: Option<f64>,
    /// Reusable input scratch, sized at arm so a fire allocates nothing.
    in_scratch: Vec<f32>,
    /// Reusable f32 drive record (the pulse arrives as f64).
    drive_scratch: Vec<f32>,
    /// **DISCRIMINATOR HOOK.** An explicit ceiling, in frames, on how
    /// much of the record [`Self::commit_whole_record`] puts in front of the
    /// clock. `None` (the default, and the only value any product path uses) =
    /// the device's own reported space. Set by the alignment probe to
    /// sweep the commit depth while holding the record fixed, which is what
    /// separates a host-side accounting delay from a device-side one.
    ///
    /// Lowering it is always SAFE for correctness (`play_block_wise` feeds the
    /// remainder after `start()`), and always costs runway: the smaller the
    /// pre-start commit, the less margin the output has before the first
    /// post-start write lands.
    pre_start_cap: Option<usize>,
    /// Frames the device took before `start()` on the LAST fire - the number
    /// `play_block_wise` was seeded with. Read by the alignment probe; kept because
    /// "how much was actually in front of the clock" is the first question every
    /// alignment investigation asks and reconstructing it from a driver log is
    /// how the last two sittings were spent.
    last_committed: usize,
}

impl<B: Backend> BackendPulseFirer<B> {
    /// Build a firer over `backend` with `table`, armed against `station`. The
    /// control channel is the FIRST enabled `Control`-role channel (slot 0
    /// fallback when none is marked). Fails on an invalid table before any
    /// backend traffic.
    pub fn new(
        backend: B,
        table: ChannelTable,
        station: eigen_module_abi::StationClass,
    ) -> Result<Self, ServiceError> {
        table
            .validate()
            .map_err(|e| ServiceError::Fire(format!("channel table: {e}")))?;
        let mut n_inputs = 0usize;
        let mut control_slot = None;
        for ch in table.channels.iter().filter(|c| c.enabled) {
            if ch.role == ChannelRole::Drive {
                continue;
            }
            if ch.role == ChannelRole::Control && control_slot.is_none() {
                control_slot = Some(n_inputs);
            }
            n_inputs += 1;
        }
        if n_inputs == 0 {
            return Err(ServiceError::Fire(
                "channel table has no enabled input channels".into(),
            ));
        }
        Ok(BackendPulseFirer {
            station,
            backend,
            table,
            n_inputs,
            control_slot: control_slot.unwrap_or(0),
            quiet_frames: 0,
            block_frames: 1,
            requested_block: None,
            actual_fs_hz: None,
            in_scratch: Vec::new(),
            drive_scratch: Vec::new(),
            pre_start_cap: None,
            last_committed: 0,
        })
    }

    /// **DISCRIMINATOR HOOK.** Cap the pre-start commit at `frames`.
    /// See [`Self::pre_start_cap`]; `None` restores the device-space default.
    pub fn with_pre_start_cap(mut self, frames: Option<usize>) -> Self {
        self.pre_start_cap = frames;
        self
    }

    /// Frames committed in front of the clock on the last fire.
    pub fn last_committed_frames(&self) -> usize {
        self.last_committed
    }

    /// Override the acquisition block this firer REQUESTS at configure. What it
    /// actually runs still comes from `ConfiguredActual`.
    pub fn with_block_frames(mut self, frames: usize) -> Self {
        self.requested_block = Some(frames.max(1));
        self
    }

    /// The block this firer is streaming at (valid after `arm()`).
    pub fn block_frames(&self) -> usize {
        self.block_frames
    }

    /// Override the quiet lead-in/lead-out length, in frames. The service
    /// normally sets this from the pulse duration and the ADOPTED rate; this is
    /// the explicit override.
    pub fn with_quiet_frames(mut self, n: usize) -> Self {
        self.quiet_frames = n;
        self
    }

    /// The quiet lead-in/lead-out this firer is surrounding pulses with, frames.
    pub fn quiet_frames(&self) -> usize {
        self.quiet_frames
    }

    /// The wrapped backend (for post-run inspection in tests).
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Commit as much of `self.drive_scratch` as the device will take while the
    /// task is NOT running, honoring the accepted count. Returns the frames the
    /// device took.
    ///
    /// It stops on the first zero-accept: nothing is draining the buffer (the
    /// task is parked), so a zero-accept means FULL, not slow. In the normal case
    /// a shock record is shorter than the output buffer and the whole thing goes
    /// in, which is what puts the backend's start-time silence top-up BEHIND the
    /// record instead of ahead of the leading edge.
    ///
    /// # `Some(0)` before the first write is NOT "full"
    ///
    /// `Backend::output_space` renders "I could not query the device" as
    /// `Some(0)` on purpose - for a RUNNING task the conservative answer is
    /// "do not top up". This function runs on a PARKED task whose buffer is
    /// empty by construction (`stop()` unreserved it, `arm()` re-committed it),
    /// so `Some(0)` here can only be the unknown, never the full. Reading it as
    /// full is what made this function a NO-OP for the life of the parked-task shape:
    /// nothing was committed, `start()` found an empty buffer and topped it up
    /// with a full lookahead of SILENCE, and every record played 2048 frames
    /// (40 ms) late behind it. Before anything has been accepted, the space
    /// query is therefore ADVISORY ONLY and the authority is `write_drive`'s own
    /// accept count - the same asymmetry `write_drive` already documents.
    fn commit_whole_record(&mut self) -> Result<usize, ServiceError> {
        // Pre-start, DAQmx ERRORS (-200293) on a write larger than the free
        // buffer rather than accepting a prefix - measured on the chassis in
        // the rate assay, 2026-08-08. So each write is capped at the
        // device's own reported space, and a record longer than the buffer
        // commits what fits; `play_block_wise` feeds the remainder AFTER
        // start() via the normal write loop (committed_frames pairs the read
        // side). A MIL-compliant compensated record routinely exceeds
        // the buffer - shallow shoulders inside the +/-15% surround band make
        // the composite ~10x the main-pulse duration - so this is the normal
        // shape, not an edge case.
        let total = self.drive_scratch.len();
        let mut done = 0usize;
        let block = self.block_frames.max(1);
        let space_before = self.backend.output_space();
        // The ONE-BLOCK PRE-START RESERVE IS GONE (it landed as a
        // candidate fix for a "full buffer at start delays the output"
        // hypothesis). The commit-depth sweep REFUTED that hypothesis
        // outright: commit caps of 512 / 1024 / 2048 / uncapped all produced the
        // identical -2370 lag, because all four committed ZERO. A reserve kept
        // for a refuted reason is runway given away for nothing, so the design
        // intent is restored - the whole record goes in front of the clock, and
        // the only cap is the device's own reported space per write (pre-start,
        // DAQmx ERRORS -200293 on a write larger than the free buffer rather
        // than accepting a prefix; measured in the rate assay).
        let pre_start_cap = self.pre_start_cap;
        while done < total {
            if let Some(cap) = pre_start_cap {
                if done >= cap {
                    break;
                }
            }
            let space = match self.backend.output_space() {
                // No space report (Sim and mocks): the old shape - offer the
                // remainder, stop on a zero accept.
                None => total - done,
                // Nothing accepted yet: the query is the unknown, not the full.
                // Offer ONE block and let the accept count answer.
                Some(0) if done == 0 => block.min(total),
                Some(0) => break,
                Some(s) => s.min(total - done),
            };
            let space = match pre_start_cap {
                Some(cap) => space.min(cap.saturating_sub(done)),
                None => space,
            };
            if space == 0 {
                break;
            }
            let accepted = self
                .backend
                .write_drive(&self.drive_scratch[done..done + space])
                .map_err(hal_err)?;
            if accepted == 0 {
                break;
            }
            done += accepted.min(total - done);
        }
        self.last_committed = done;
        if std::env::var_os("EIGEN_NI_DIAG").is_some() {
            eprintln!(
                "[shock-diag] commit_whole_record: record {total} frames, space-before {:?}, \
                 block {}, cap {pre_start_cap:?} -> COMMITTED {done} frames before start()",
                space_before, self.block_frames
            );
        }
        Ok(done)
    }
}

impl<B: Backend> PulseFirer for BackendPulseFirer<B> {
    fn path(&self) -> DrivePath {
        DrivePath::HostDac
    }

    fn actual_fs_hz(&self) -> Option<f64> {
        self.actual_fs_hz
    }

    fn set_quiet_frames(&mut self, frames: usize) {
        self.quiet_frames = frames;
    }

    /// Open, configure, ADOPT the rate, consult the gate, verify bring-up, and
    /// PARK.
    ///
    /// **Nothing starts here.** The earlier shape called `start()` at arm, which
    /// bought two measured defects at once: `NiBackend::start()` prefills the
    /// output with SILENCE when the caller has queued less than one lookahead,
    /// so the arm-time start put 2048 frames of silence exactly where the first
    /// pulse's leading edge belonged (2026-08-08 baseline: the whole 624-frame
    /// record sat BEHIND it and the probe found no correlation peak at all); and
    /// the input task ran from arm with nothing draining it, so an operator who
    /// held armed for a few seconds hit a `-200279` overflow or the AO
    /// idle-starvation the baseline actually measured ("drive output accepted
    /// nothing after 0 frames"). Parked tasks cannot do either.
    fn arm(&mut self) -> Result<(), ServiceError> {
        self.backend.open().map_err(hal_err)?;
        let requested = self.backend.caps().sample_rate_hz;
        let want_block = self
            .requested_block
            .unwrap_or_else(|| eigen_hal::default_block_frames(requested));
        let acq = eigen_hal::AcquisitionConfig::new(requested, want_block);
        let actual = self.backend.configure(&self.table, &acq).map_err(hal_err)?;

        // THE ADOPTION, and it happens BEFORE the gate on purpose (the arm
        // order is open -> configure -> ADOPT -> arm_gate(..., Some(fs_POST_adoption),
        // actual)). Everything
        // Hz-denominated in this crate reads back through `actual_fs_hz()`.
        if !(actual.actual_rate_hz.is_finite() && actual.actual_rate_hz > 0.0) {
            return Err(ServiceError::Fire(format!(
                "backend configured a non-usable rate ({} Hz); nothing can be denominated \
                 against it",
                actual.actual_rate_hz
            )));
        }
        self.actual_fs_hz = Some(actual.actual_rate_hz);

        // THE ARM GATE. Path B (HostDac) requests
        // `{HostSynthesized, HostBetweenRuns}`: the host plays the pulse and
        // its compensation law recomputes the drive BETWEEN shots. Note the
        // contrast with Path A (Delegated), which is `Device` adaptation -
        // there the host's level ladder is a SEQUENCER (it picks the next
        // setpoint but never recomputes the drive from the measured response),
        // and Path A deliberately downloads an UNCOMPENSATED reference so the
        // device's own compensation is not doubled. That line - "does the host
        // recompute the drive from the response" - is what separates `Device`
        // from `HostBetweenRuns`, and the two look identical from a distance.
        //
        // The factory supplies the class; it is held on the firer from
        // construction so the gate reads the SAME value the caller stated.
        //
        // WHAT THE RATE ARGUMENT IS, stated honestly: the profile side is the
        // rate this firer JUST ADOPTED, so post-adoption the two sides are the
        // same number and the rate band cannot red here. That is a
        // deliberate order and it still does work - `None` is what Random
        // shipped once, and it put every profile frequency 6.25x high on the
        // first chassis run (2026-07-30). The check that CAN red on a real
        // disagreement is the service's: it compares the operator's REQUESTED
        // `cfg.fs_hz` against this adopted rate and logs the coercion, because
        // refusing there would refuse this chassis outright (5120 requested,
        // 51 200 delivered).
        eigen_module_abi::arm_gate(
            &self.backend.caps(),
            self.station,
            eigen_module_abi::Resolution::host_iterative(),
            self.actual_fs_hz,
            actual.actual_rate_hz,
        )
        .map_err(|reason| ServiceError::Fire(format!("arm refused: {reason}")))?;

        self.block_frames = actual.block_frames.max(1);
        self.in_scratch = vec![0.0f32; self.block_frames * self.n_inputs];

        // Bring-up verification, then PARK. `arm()` is the driver-side "does
        // this configuration commit" check; `stop()` puts both tasks back down
        // and UNRESERVES them, which is the state every fire starts from.
        self.backend.arm().map_err(hal_err)?;
        self.backend.stop().map_err(hal_err)?;
        Ok(())
    }

    /// ONE pulse, on its own cold task.
    ///
    /// # The cold-task invariant
    ///
    /// **The AO write cursor and the AI read cursor are aligned by construction
    /// at every fire.** Both tasks are parked between shots, so `start()` is the
    /// single instant at which output and input begin, and the first clocked
    /// output sample is the first sample of this record. Nothing accumulates
    /// across shots: not a silence prefill in front of the pulse, not the
    /// inter-pulse wall time that used to slide the measured record away from
    /// the pulse by however long the operator took to press Fire.
    ///
    /// # The order, and why each step is where it is
    ///
    /// 1. `arm()` - commit the tasks from parked.
    /// 2. **Commit the WHOLE record** before the clock runs.
    ///    `NiBackend::start()` tops the output buffer up with SILENCE when less
    ///    than one lookahead is queued; anything it adds lands BEHIND what is
    ///    already there. Committing the record first is what makes the top-up a
    ///    tail pad instead of a head pad. A record SHORTER than the output buffer
    ///    is the normal case (an 11 ms pulse at 51.2 kHz with its compensation
    ///    shoulders is a few thousand frames) and a record LONGER than it needs no
    ///    special handling either: the device takes what it has room for and the
    ///    play loop re-offers the rest, in order.
    /// 3. `start()` - output and input release together.
    /// 4. `play_block_wise` over the WHOLE record, told how many frames are
    ///    already at the device (`committed_frames`). It skips the
    ///    committed prefix on the write side and still reads back exactly the
    ///    record length, so nothing is re-sent and nothing falls out of the
    ///    response window.
    /// 5. `stop()` - park + unreserve. Its shutdown taper is a documented NO-OP
    ///    here: the last frame written is one of the trailing zeros, so the ramp
    ///    starts from 0.0 and ends at 0.0 ("Stop is ramped" is vacuous
    ///    for a transient producer, and saying so is the point).
    ///
    /// Until 2026-08-08 steps 2, 4 and 5 were a workaround for a missing input on
    /// the shared stream helper: preload only `min_lookahead_frames`, pad the
    /// record's tail by that much, play a WINDOW offset by the prefill, then read
    /// and discard the same number of input frames at the far end to rebalance the
    /// cursors. Every part of that existed because `play_block_wise` could not be
    /// told what the device already held. It now has that parameter; the
    /// workaround is gone and the shape is now the same as the SRS plant's.
    fn fire(
        &mut self,
        drive_g: &[f64],
        _commanded_db: f64,
        ctl: &mut FireControl<'_>,
    ) -> Result<Option<Vec<f64>>, ServiceError> {
        let q = self.quiet_frames;
        let n_inputs = self.n_inputs;
        let control_slot = self.control_slot;

        // THE RECORD: quiet lead-in, the pulse, quiet lead-out. This is what is
        // measured, and its length is what the play loop must read back.
        let record_frames = drive_g.len() + 2 * q;
        if record_frames == 0 {
            return Err(ServiceError::Fire(
                "the commanded pulse is empty; there is nothing to fire".into(),
            ));
        }
        self.drive_scratch.clear();
        self.drive_scratch.reserve(record_frames);
        self.drive_scratch.resize(q, 0.0);
        self.drive_scratch.extend(drive_g.iter().map(|&d| d as f32));
        self.drive_scratch.resize(record_frames, 0.0);

        // Scratch is sized at arm; a firer used before arm has none.
        if self.in_scratch.is_empty() {
            self.in_scratch = vec![0.0f32; self.block_frames.max(1) * n_inputs];
        }

        // 1 + 2 + 3: cold task, the whole record in front of the clock, then
        // release.
        self.backend.arm().map_err(hal_err)?;
        let committed = self.commit_whole_record()?;
        self.backend.start().map_err(hal_err)?;

        // 4: play the record, reading back exactly `record_frames`.
        let mut measured = Vec::with_capacity(record_frames);
        let play = eigen_hal::BlockPlay::new(self.block_frames);
        let total = record_frames;
        let mut delivered = 0usize;
        let cancel = ctl.cancel;
        let observer = &mut ctl.observer;
        let played = eigen_hal::play_block_wise(
            &mut self.backend,
            &self.drive_scratch,
            1,
            n_inputs,
            &mut self.in_scratch,
            &play,
            committed,
            cancel,
            |block, frames| {
                for f in 0..frames {
                    measured.push(block[f * n_inputs + control_slot] as f64);
                }
                delivered += frames;
                if let Some(obs) = observer.as_mut() {
                    obs(delivered, total);
                }
            },
        )
        .map_err(stream_err);

        // 5: park. It runs on the cancel path too - a cancelled fire must not
        // leave a started task behind it.
        let stopped = self.backend.stop().map_err(hal_err);
        played?;
        stopped?;
        Ok(Some(measured))
    }

    fn take_warnings(&mut self) -> Vec<String> {
        self.backend.take_warnings()
    }

    fn stop(&mut self) -> Result<(), ServiceError> {
        self.backend.stop().map_err(hal_err)
    }

    fn emergency_stop(&mut self) -> Result<(), ServiceError> {
        self.backend.emergency_stop().map_err(hal_err)
    }
}
