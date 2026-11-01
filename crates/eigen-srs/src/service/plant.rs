//! The fire-path seams for the SRS-shock service: the Path-B host
//! [`PulsePlant`] (play a synthesized pulse into a HAL backend, capture the
//! responses) and the Path-A [`DelegatedShockDevice`] (download the reference once,
//! fire absolute scalars).
//!
//! # Path honesty
//!
//! The two seams are DIFFERENT TRAITS on purpose, mirroring
//! [`eigen_shock_common::DrivePath`]'s capability split:
//!
//! - **Path B ([`PulsePlant`])**: the host owns the whole loop. Every pulse is
//!   synthesized on the host, played sample-by-sample into the backend's drive, and
//!   the control + monitor responses are captured back. [`HalPulsePlant`] adapts
//!   any [`eigen_hal::Backend`] (the end-to-end fixtures use `eigen-hal-sim`'s
//!   `SimBackend` resonance plant; direct-DAC and NI backends are the gated
//!   real-hardware homes).
//! - **Path A ([`DelegatedShockDevice`])**: Eigen downloads the synthesized
//!   reference ONCE and fires absolute `10^(dB/20)` scalars; the DEVICE closes the
//!   loop. Eigen's control law does NOT run there, and the download is
//!   the UNCOMPENSATED synthesis (the device compensates on-device).
//!   There is no per-pulse host drive to notch, no host system ID,
//!   and no V/I observability.

use eigen_channel_table::{ChannelRole, ChannelTable};
use eigen_hal::Backend;
use std::sync::atomic::AtomicBool;

/// How one [`PulsePlant::fire`] failed.
///
/// Typed rather than a `String` because the service has to tell ONE outcome apart
/// from every other: a play the operator CANCELLED is not a fault, and routing it
/// through the plant-trip path would write a `TripKind::Plant` record for a
/// deliberate stop.
#[derive(Debug, Clone, PartialEq)]
pub enum PlantError {
    /// The caller's cancel flag was observed TRUE at a block boundary, so the play
    /// returned early. Carries [`eigen_hal::StreamError::Cancelled`]'s arithmetic
    /// verbatim: `frames_committed - frames_done` frames are still queued AT THE
    /// DEVICE and keep playing (mid-record preemption is refused by the cDAQ-9178,
    /// `-200289`). The CALLER decides what happens to that tail - see
    /// [`PulsePlant::stop`] (let it drain) vs [`PulsePlant::emergency_stop`]
    /// (taper and kill).
    Cancelled {
        frames_done: usize,
        frames_committed: usize,
        frames_expected: usize,
    },
    /// Bring-up, driver, or stream failure. The message is the underlying error.
    Failed(String),
}

impl std::fmt::Display for PlantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlantError::Cancelled {
                frames_done,
                frames_committed,
                frames_expected,
            } => write!(
                f,
                "fire CANCELLED after {frames_done} of {frames_expected} frames; \
                 {} committed frame(s) still queued at the device",
                frames_committed.saturating_sub(*frames_done)
            ),
            PlantError::Failed(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for PlantError {}

impl From<eigen_hal::StreamError> for PlantError {
    fn from(e: eigen_hal::StreamError) -> Self {
        match e {
            eigen_hal::StreamError::Cancelled {
                frames_done,
                frames_committed,
                frames_expected,
            } => PlantError::Cancelled {
                frames_done,
                frames_committed,
                frames_expected,
            },
            other => PlantError::Failed(other.to_string()),
        }
    }
}

/// One fired pulse's captured responses (Path B).
#[derive(Debug, Clone, PartialEq)]
pub struct FiredResponses {
    /// The control-channel acceleration record, g (drive samples + ring-out).
    pub control_g: Vec<f64>,
    /// Monitor-channel records `(name, samples)` in each channel's engineering
    /// unit, same length as `control_g`. The name is the channel-table
    /// `description` (or `"ch<index>"` when the description is empty) - the string
    /// the [`eigen_shock_common::DriveMonitor::channel`] field matches against.
    pub monitors: Vec<(String, Vec<f64>)>,
}

/// The Path-B host fire seam: play one synthesized drive record through the plant
/// and capture the responses. Implementations own the backend bring-up.
///
/// # The COLD-TASK shape
///
/// The seam is per-shot cold on purpose. [`Self::ensure_open`] verifies bring-up
/// and then PARKS: no device task runs between arm and a fire, and none runs
/// between two fires. Each [`Self::fire`] arms, commits the WHOLE record, starts,
/// plays, and stops. Three defects die with the old always-running shape:
///
/// * the start-time silence prefill no longer sits AHEAD of the pulse (the device
///   tops the output buffer up BEHIND a record that is already committed);
/// * an armed-idle or review-hold pause can no longer overrun the input ring;
/// * the output write cursor and the input read cursor are aligned BY
///   CONSTRUCTION at every fire, instead of drifting by the accumulated
///   inter-pulse wall time.
///
/// # Teardown is part of the contract
///
/// [`Self::stop`] and [`Self::emergency_stop`] are what the SERVICE's terminal
/// paths call. Before they existed, an SRS `stop()` or envelope abort changed a
/// state enum and closed a capture file while the device task stayed reserved and
/// running - the zombie-AO class measured on this chassis 2026-08-03.
///
/// A [`Self::fire`] that returns `Err` leaves the device task RUNNING on purpose:
/// only the caller knows whether the right teardown is a taper-and-drain
/// ([`Self::stop`]) or an immediate kill ([`Self::emergency_stop`]), so the fire
/// path must not pre-empt that choice. Every error exit therefore obliges the
/// caller to park the plant.
pub trait PulsePlant {
    /// The plant sample rate, Hz (the synthesis runs at this rate).
    ///
    /// **Two different numbers, before and after [`Self::ensure_open`].** Before,
    /// this is the REQUESTED rate (what the caller asked the device for). After,
    /// it is the rate the device actually configured, which on a cDAQ-9234 is
    /// coerced up its `13.1072 MHz / (256 n)` ladder and is routinely not what was
    /// asked for. The service re-derives every Hz-denominated quantity it owns
    /// from this value at arm; reading it before `ensure_open` and
    /// caching the answer is the rate-divorce defect.
    fn fs_hz(&self) -> f64;

    /// Bring the plant up (idempotent) and PARK it. Called at arm.
    ///
    /// Opens, configures, ADOPTS the configured rate, runs the arm gate, and arms
    /// the device task. It deliberately does NOT start: see the trait docs.
    fn ensure_open(&mut self) -> Result<(), String>;

    /// Play `drive_g` into the drive output, then keep reading for
    /// `ring_out_samples` more frames with zero drive (the fixture ring-down), and
    /// return every input channel's record over the whole window.
    ///
    /// `cancel`, when `Some`, is read once per block BEFORE more drive is offered;
    /// observed `true` returns [`PlantError::Cancelled`] with the committed-queue
    /// arithmetic. `None` never reads a flag.
    fn fire(
        &mut self,
        drive_g: &[f64],
        ring_out_samples: usize,
        cancel: Option<&AtomicBool>,
    ) -> Result<FiredResponses, PlantError>;

    /// PARK the plant: stop the device task and unreserve it, leaving the plant
    /// re-armable by the next [`Self::fire`]. The clean-stop teardown
    /// ("complete the current pulse, cancel at the next inter-pulse boundary,
    /// park"), and what entering the review hold runs so nothing stays
    /// reserved-and-running through a minutes-long operator pause.
    ///
    /// Returns the underlying failure rather than swallowing it: a teardown that
    /// fails silently is the same class of defect as a driver warning collected
    /// where nobody can read it. Callers on a terminal path LOG it and continue -
    /// a failed park must never block reaching a safe state.
    fn stop(&mut self) -> Result<(), String>;

    /// EMERGENCY teardown: taper and kill the task now. The abort path.
    ///
    /// Latency is bounded BELOW by what the device has already committed, not by
    /// this call: mid-record preemption is refused by the cDAQ-9178 (`-200289`),
    /// so the committed queue (~60 ms on this chassis, which is longer than a
    /// whole shock pulse) plays out regardless. "Kill the drive mid-record" does
    /// not exist on this hardware.
    fn emergency_stop(&mut self) -> Result<(), String>;
}

/// The Path-A delegated fire seam: download once, fire scalars.
pub trait DelegatedShockDevice {
    /// The device sample rate, Hz (the reference is synthesized at this rate).
    fn fs_hz(&self) -> f64;

    /// Download the UNCOMPENSATED reference (the device compensates
    /// on-device; a host-compensated download would double-compensate).
    fn download_reference(&mut self, reference_g: &[f64]) -> Result<(), String>;

    /// Fire one pulse at the absolute linear scalar `10^(dB/20)` and return the
    /// measured control-channel record, g (pulse capture exists on Path A too, so
    /// every pulse is recorded on both paths).
    fn fire_scalar(&mut self, scale: f64) -> Result<Vec<f64>, String>;
}

/// The fire path a service instance runs on. Wraps the path-appropriate seam; the
/// service reads its capability truth from
/// [`eigen_shock_common::PathCapabilities`], never from this enum's shape.
///
/// # Why the trait objects are `+ Send`
///
/// Nothing in either seam is genuinely `!Send`: [`HalPulsePlant`] holds a
/// `B: Backend`, and `Backend: Send` is a supertrait, so the ONE existing
/// implementor is `Send` already; `DelegatedShockDevice` has ZERO implementors.
/// The missing bound was the whole story, and it made `SrsShockService` `!Send`
/// for no reason a type could name - which in turn forced every host to confine
/// the service to a dedicated thread. The bound costs one word each and is
/// checked by the compiler at every `FirePath::Host(Box::new(..))` site.
pub enum FirePath {
    /// Path B: host closes the loop.
    Host(Box<dyn PulsePlant + Send>),
    /// Path A: delegated reference download + scalar fires.
    Delegated(Box<dyn DelegatedShockDevice + Send>),
}

impl FirePath {
    /// The path discriminant for the capability table.
    pub fn drive_path(&self) -> eigen_shock_common::DrivePath {
        match self {
            FirePath::Host(_) => eigen_shock_common::DrivePath::HostDac,
            FirePath::Delegated(_) => eigen_shock_common::DrivePath::Delegated,
        }
    }

    /// The fire-path sample rate. On Path B this is the REQUESTED rate until
    /// [`PulsePlant::ensure_open`] has run and the ACTUAL configured rate after
    /// (see [`PulsePlant::fs_hz`]).
    pub fn fs_hz(&self) -> f64 {
        match self {
            FirePath::Host(p) => p.fs_hz(),
            FirePath::Delegated(d) => d.fs_hz(),
        }
    }

    /// Park the fire path. A no-op on Path A: the DEVICE owns the task
    /// there, and Eigen has no teardown verb for it.
    pub fn stop(&mut self) -> Result<(), String> {
        match self {
            FirePath::Host(p) => p.stop(),
            FirePath::Delegated(_) => Ok(()),
        }
    }

    /// Emergency-park the fire path. A no-op on Path A, same reason as
    /// [`Self::stop`].
    pub fn emergency_stop(&mut self) -> Result<(), String> {
        match self {
            FirePath::Host(p) => p.emergency_stop(),
            FirePath::Delegated(_) => Ok(()),
        }
    }
}

/// The display name for a channel: its `description`, or `"ch<index>"` when the
/// description is empty. This is the string [`FiredResponses::monitors`] keys by and
/// the [`eigen_shock_common::DriveMonitor::channel`] / limit-channel names match.
pub fn channel_display_name(index: u32, description: &str) -> String {
    let d = description.trim();
    if d.is_empty() {
        format!("ch{index}")
    } else {
        d.to_string()
    }
}

/// A [`PulsePlant`] over any [`eigen_hal::Backend`]: opens/configures/arms/starts
/// the backend from a channel table, then plays each pulse BLOCK-WISE through
/// [`eigen_hal::play_block_wise`].
///
/// It used to step one sample per call (`write_drive` one sample, `read_input`
/// one frame), the shape the retired `SimBackend` drive latch forced. Two things
/// changed with the move to blocks, and the second is a BUG FIX:
///
/// * a write now honors `write_drive`'s ACCEPTED count and re-offers the
///   remainder, and a read POLLS a `NoDataYet` against a deadline instead of
///   being an instant fatal;
/// * **`read_input`'s `InputRead` variant is no longer DISCARDED.** The old fire
///   loop wrote `self.backend.read_input(&mut frame).map_err(...)?;` and then
///   pushed `frame` into the response record unconditionally. On `NoDataYet` the
///   backend writes NOTHING, so `frame` still held the PREVIOUS frame's samples -
///   and the loop recorded them again as if they were a fresh measurement. That
///   is the stale-buffer-as-measurement bug class in its purest form: a duplicated
///   sample is indistinguishable from a real one downstream, and on a shock pulse
///   it silently distorts the very peak the SRS is computed from.
///   `play_block_wise` hands back ONLY frames the backend actually delivered.
///
/// Channel mapping mirrors the backend convention: enabled `Control` / `Monitor` /
/// `Reference` channels are the input frame slots in TABLE ORDER; the FIRST enabled
/// `Control` channel is the control record; every OTHER input channel is returned
/// as a named monitor record.
pub struct HalPulsePlant<B: Backend> {
    /// The station class the arm gate is given. Supplied
    /// by whoever built the backend, because the class is a property of what the
    /// backend is bound to, not of `B`.
    station: eigen_module_abi::StationClass,
    backend: B,
    table: ChannelTable,
    opened: bool,
    /// Input-frame slot of the control channel.
    control_slot: usize,
    /// `(name, slot)` of every non-control input channel.
    monitor_slots: Vec<(String, usize)>,
    /// The rate the synthesis runs at. The REQUESTED rate (from `caps()`) until
    /// `ensure_open` runs; the rate the device actually CONFIGURED after.
    fs_hz: f64,
    /// What was asked for, kept so the divergence has a name in the report rather
    /// than vanishing into the adopted value.
    requested_fs_hz: f64,
    /// Frames per write/read block, from what the backend CONFIGURED. Set in
    /// `ensure_open`; 1 until then.
    block_frames: usize,
    /// The block this plant REQUESTS at configure. `None` = derive it from the
    /// rate (`eigen_hal::default_block_frames`).
    requested_block: Option<usize>,
    /// Reusable input scratch, sized at open so a fire allocates nothing.
    in_scratch: Vec<f32>,
    /// Reusable f32 drive block (the pulse record arrives as f64).
    drive_scratch: Vec<f32>,
    /// The device task is ARMED and parked (cold-task shape). `ensure_open`
    /// leaves it true; a fire's `start()` consumes it and the fire's `stop()` leaves
    /// it false, so the next fire re-arms. Tracked rather than re-derived because
    /// `Backend::arm()` REFUSES from `Armed` - "arm twice" is an error, not a no-op.
    armed: bool,
}

impl<B: Backend> HalPulsePlant<B> {
    /// Build the adapter, armed against `station`. Validates that the table has
    /// at least one enabled `Control` channel (the loop needs a control point).
    /// The backend is NOT touched until [`PulsePlant::ensure_open`].
    pub fn new(
        backend: B,
        table: ChannelTable,
        station: eigen_module_abi::StationClass,
    ) -> Result<Self, String> {
        let fs_hz = backend.caps().sample_rate_hz;
        let mut control_slot: Option<usize> = None;
        let mut monitor_slots = Vec::new();
        let mut slot = 0usize;
        for ch in table.channels.iter().filter(|c| c.enabled) {
            match ch.role {
                ChannelRole::Drive => {}
                ChannelRole::Control | ChannelRole::Monitor | ChannelRole::Reference => {
                    if ch.role == ChannelRole::Control && control_slot.is_none() {
                        control_slot = Some(slot);
                    } else {
                        monitor_slots.push((channel_display_name(ch.index, &ch.description), slot));
                    }
                    slot += 1;
                }
            }
        }
        let control_slot = control_slot
            .ok_or_else(|| "channel table has no enabled Control channel".to_string())?;
        Ok(HalPulsePlant {
            station,
            backend,
            table,
            opened: false,
            control_slot,
            monitor_slots,
            fs_hz,
            requested_fs_hz: fs_hz,
            block_frames: 1,
            requested_block: None,
            in_scratch: Vec::new(),
            drive_scratch: Vec::new(),
            armed: false,
        })
    }

    /// The rate this plant ASKED the device for (its `caps().sample_rate_hz` at
    /// construction). Equal to [`PulsePlant::fs_hz`] until `ensure_open` adopts
    /// whatever the device actually configured.
    pub fn requested_fs_hz(&self) -> f64 {
        self.requested_fs_hz
    }

    /// Override the acquisition block this plant REQUESTS at configure. What it
    /// actually runs still comes from `ConfiguredActual`.
    pub fn with_block_frames(mut self, frames: usize) -> Self {
        self.requested_block = Some(frames.max(1));
        self
    }

    /// The block this plant is streaming at (valid after `ensure_open`).
    pub fn block_frames(&self) -> usize {
        self.block_frames
    }

    /// The wrapped backend (for tests / teardown).
    pub fn backend(&self) -> &B {
        &self.backend
    }

    fn n_input_channels(&self) -> usize {
        1 + self.monitor_slots.len()
    }
}

impl<B: Backend> PulsePlant for HalPulsePlant<B> {
    fn fs_hz(&self) -> f64 {
        self.fs_hz
    }

    fn ensure_open(&mut self) -> Result<(), String> {
        if self.opened {
            return Ok(());
        }
        self.backend.open().map_err(|e| e.to_string())?;
        let want_block = self
            .requested_block
            .unwrap_or_else(|| eigen_hal::default_block_frames(self.fs_hz));
        let acq = eigen_hal::AcquisitionConfig::new(self.fs_hz, want_block);
        let actual = self
            .backend
            .configure(&self.table, &acq)
            .map_err(|e| e.to_string())?;

        // ADOPT THE DEVICE RATE. `ConfiguredActual::actual_rate_hz` is the authority from
        // configure onward and this seam used to DISCARD it: `self.fs_hz` stayed at
        // `caps().sample_rate_hz` - the pre-configure build rate - so every subsequent
        // synthesis, ring-out count and SRS analysis ran on a timebase the device was not
        // playing at (the rate-divorce class). SRS synthesizes the wavelet drive fresh for
        // every shot, so adopting is strictly correct here: there is no pre-generated
        // record to invalidate, and REFUSING a coerced rate (the 9234 rounds every request
        // up its 13.1072 MHz/(256 n) ladder) would refuse every run on this chassis. The
        // service re-derives its own Hz-denominated set from `fs_hz()` at arm.
        if !(actual.actual_rate_hz.is_finite() && actual.actual_rate_hz > 0.0) {
            return Err(format!(
                "device configured a bad sample rate ({} Hz)",
                actual.actual_rate_hz
            ));
        }
        self.fs_hz = actual.actual_rate_hz;

        // THE ARM GATE. SRS requests `{HostSynthesized, HostBetweenRuns}`: the
        // host synthesizes the wavelet drive and its correction law runs
        // BETWEEN shots. No shared-clock requirement (each shot is
        // independent), so SRS-on-NI is not gated behind the arrival-day clock
        // bench result.
        //
        // The factory supplies the class; it is held on the plant from
        // construction so the gate reads the SAME value the caller stated.
        //
        // The rate argument is the POST-ADOPTION rate, in the non-negotiable arm
        // order (open -> configure -> adopt -> gate with
        // `Some(fs_post_adoption)`, NEVER `None`; Random shipped `None` once and put
        // every profile frequency 6.25x high). Be precise about what that makes this
        // check: an assertion that adoption ACTUALLY HAPPENED, not a comparison of
        // two independent authorities. It cannot fail while the line above runs, and
        // a check that cannot fail is not evidence. The rate claims
        // that CAN fail live in the service: the Nyquist re-check against the adopted
        // rate at arm, and the logged requested-vs-adopted divergence carried into
        // `RunSettings`.
        eigen_module_abi::arm_gate(
            &self.backend.caps(),
            self.station,
            eigen_module_abi::Resolution::host_iterative(),
            Some(self.fs_hz),
            actual.actual_rate_hz,
        )
        .map_err(|reason| format!("arm refused: {reason}"))?;

        // Verify bring-up, then PARK. `arm()` is the last lifecycle step
        // taken here: nothing STARTS until a fire has the whole record committed.
        // Starting at arm is what put the device's own silence prefill AHEAD of
        // every pulse and left the input ring filling with nobody reading it
        // through an armed-idle pause (the `-200279` overrun).
        self.backend.arm().map_err(|e| e.to_string())?;
        self.armed = true;
        self.block_frames = actual.block_frames.max(1);
        let n_ch = self.n_input_channels();
        self.in_scratch = vec![0.0f32; self.block_frames * n_ch];
        self.opened = true;
        Ok(())
    }

    fn fire(
        &mut self,
        drive_g: &[f64],
        ring_out_samples: usize,
        cancel: Option<&AtomicBool>,
    ) -> Result<FiredResponses, PlantError> {
        if !self.opened {
            return Err(PlantError::Failed("plant not open (arm first)".into()));
        }
        // COLD TASK, per shot. Re-arm if the previous fire's stop parked
        // the task; the very first fire after `ensure_open` is already armed.
        if !self.armed {
            self.backend
                .arm()
                .map_err(|e| PlantError::Failed(e.to_string()))?;
            self.armed = true;
        }

        let n_ch = self.n_input_channels();
        let total = drive_g.len() + ring_out_samples;
        let mut control = Vec::with_capacity(total);
        let mut monitors: Vec<(String, Vec<f64>)> = self
            .monitor_slots
            .iter()
            .map(|(name, _)| (name.clone(), Vec::with_capacity(total)))
            .collect();

        // The full record: the pulse, then `ring_out_samples` of zero drive so the
        // fixture ring-down is captured on the same timebase.
        self.drive_scratch.clear();
        self.drive_scratch.reserve(total);
        self.drive_scratch.extend(drive_g.iter().map(|&d| d as f32));
        self.drive_scratch.resize(total, 0.0);

        // COMMIT THE RECORD'S HEAD BEFORE `start()`, capped at the device's own
        // reported space. Pre-start, DAQmx ERRORS (-200293) on a write larger
        // than the free buffer rather than accepting a prefix (measured on the
        // chassis in a rate-assay row, 2026-08-08) - so each write is
        // capped, and a record longer than the buffer commits what fits; the
        // play loop below feeds the remainder AFTER start() (committed_frames
        // pairs the read side). MIL-compliant shallow compensation makes
        // records-longer-than-buffer the NORMAL case, not an edge.
        //
        // `Some(0)` BEFORE THE FIRST WRITE IS THE UNKNOWN, NOT A FULL DEVICE.
        // `Backend::output_space` renders a failed space query as `Some(0)` - the
        // conservative answer for a RUNNING task. This loop runs on a PARKED task whose
        // buffer is empty by construction, so reading it as "full" commits NOTHING, and
        // `start()` then fills the buffer with a whole lookahead of SILENCE that the record
        // plays behind. Measured on the cDAQ-9178 2026-08-08 in the classical firer's
        // identical loop: every record 2048 frames (40 ms) late, bit-stable. Before
        // anything is accepted the query is ADVISORY and `write_drive`'s accept count is
        // the authority. (The one-block pre-start reserve that used to sit here went with
        // the hypothesis it was built for - a commit-depth sweep refuted it.)
        let mut committed = 0usize;
        let block = self.block_frames.max(1);
        while committed < total {
            let space = match self.backend.output_space() {
                None => total - committed,
                Some(0) if committed == 0 => block.min(total),
                Some(0) => break,
                Some(s) => s.min(total - committed),
            };
            if space == 0 {
                break;
            }
            let accepted = self
                .backend
                .write_drive(&self.drive_scratch[committed..committed + space])
                .map_err(|e| PlantError::Failed(e.to_string()))?;
            if accepted == 0 {
                break;
            }
            committed += accepted.min(total - committed);
        }

        // START. Any silence the backend tops the buffer up with now lands BEHIND
        // the committed record, so the first clocked sample is real drive - the
        // whole point of the cold-task shape.
        self.backend
            .start()
            .map_err(|e| PlantError::Failed(e.to_string()))?;
        self.armed = false;

        let control_slot = self.control_slot;
        let monitor_slots = &self.monitor_slots;
        let play = eigen_hal::BlockPlay::new(self.block_frames);
        // TELL THE PLAY WHAT IS ALREADY AT THE DEVICE. `play_block_wise`
        // pairs its reads with its OWN writes, so the pre-start commit has to be
        // visible to it: hand it the full record with `committed_frames` and it
        // skips the committed prefix on the write side while still reading back the
        // WHOLE record. Until 2026-08-08 this crate closed the same gap with a
        // ~60-line `CommittedHead` Backend view - a delegation shim in a module
        // crate standing in for a missing parameter on the shared helper, which
        // classical shock needed too. The parameter retired both.
        let played = eigen_hal::play_block_wise(
            &mut self.backend,
            &self.drive_scratch,
            1,
            n_ch,
            &mut self.in_scratch,
            &play,
            committed,
            cancel,
            |block, frames| {
                for f in 0..frames {
                    let base = f * n_ch;
                    control.push(block[base + control_slot] as f64);
                    for (m, (_, slot)) in monitors.iter_mut().zip(monitor_slots) {
                        m.1.push(block[base + *slot] as f64);
                    }
                }
            },
        );

        // A failed or cancelled play leaves the task RUNNING deliberately: the
        // caller picks the teardown (see the trait docs).
        //
        // `StreamError::Cancelled`'s `frames_committed` needs no correction here:
        // seeded with `committed`, the helper's own count already includes the
        // pre-start commit. It used to be patched up on this line, because
        // understating the committed queue is the dangerous direction - the queue
        // is exactly what keeps driving after the flag flips, and this hardware
        // will not preempt it (`cold_task_shape.rs::e_*` is what pins it).
        played?;

        // STOP + UNRESERVE. The old code parked the drive by writing ONE zero frame
        // into a task that kept running; that left the task reserved between shots
        // (the zombie-AO precondition) and did nothing about the input ring. The
        // record already ends in `ring_out_samples` zeros, so the backend's stop
        // taper starts from 0.0 and is a measured no-op here - the park is the
        // unreserve, not the ramp.
        self.backend
            .stop()
            .map_err(|e| PlantError::Failed(e.to_string()))?;
        self.armed = false;

        Ok(FiredResponses {
            control_g: control,
            monitors,
        })
    }

    fn stop(&mut self) -> Result<(), String> {
        self.armed = false;
        self.backend.stop().map_err(|e| e.to_string())
    }

    fn emergency_stop(&mut self) -> Result<(), String> {
        self.armed = false;
        self.backend.emergency_stop().map_err(|e| e.to_string())
    }
}
