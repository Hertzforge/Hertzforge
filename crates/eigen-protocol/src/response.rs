//! Core -> client responses, one per command.

use serde::{Deserialize, Serialize};

use crate::caps::{ActionKind, BackendId, BackendInfo, CapabilityDescriptor};
use crate::docs::{CaptureInfo, Doc, TableDoc};
use crate::error::ProtocolError;

/// Identifies one in-flight converging intent. Server-assigned, unique per
/// connection; [`crate::Event::IntentProgress`] / [`crate::Event::IntentSettled`]
/// correlate on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IntentId(pub u64);

/// How an accepted command completes. THE load-bearing distinction of this seam: the
/// same intent is instant on setpoint hardware and takes seconds of paced convergence
/// on nudge hardware, and a UI that cannot tell the difference will block or look
/// broken on the paced path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completion {
    /// Applied before this response was sent. Nothing further will arrive for this
    /// command.
    Immediate,
    /// Accepted and converging in the background. Progress arrives as
    /// [`crate::Event::IntentProgress`] and the terminal state as
    /// [`crate::Event::IntentSettled`], both carrying this id.
    Converging {
        /// The intent to watch for.
        intent: IntentId,
    },
}

/// The run lifecycle, UI-facing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// No test configured.
    Idle,
    /// Profile accepted; not armed.
    Configured,
    /// Armed; ready to start.
    Armed,
    /// Running.
    Running,
    /// Controlled ramp-down in progress.
    Stopping,
    /// Ended normally.
    Stopped,
    /// Ended by abort or fault; see the status flags.
    Faulted,
}

/// One converging intent still in flight, so a UI (re)connecting mid-run can rebuild
/// its in-flight affordances instead of assuming a quiet console.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveIntent {
    /// The intent id events are correlating on.
    pub intent: IntentId,
    /// Which action it is converging.
    pub action: ActionKind,
}

/// The pull-based run status ([`crate::Command::GetStatus`]), also pushed on change
/// via [`crate::Event::StateChanged`]. Representative fields translated from the
/// existing per-module status surfaces; `Option` marks readouts that do not exist in
/// every mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusReport {
    /// The active backend, when one is selected.
    pub backend: Option<BackendId>,
    /// Lifecycle state.
    pub run: RunState,
    /// Current output level, dB relative to full level (closed-loop modes).
    pub level_db: Option<f64>,
    /// Current frequency, Hz (sine-family modes).
    ///
    /// **FILLED, as of v4, by whichever module is live and frequency-bearing** - the
    /// same authority that answers a [`crate::Command::GoToFrequency`]. It is the
    /// frequency the core is SYNTHESIZING (or, on paced hardware, its best telemetry
    /// estimate), never the last value a front end requested: a readout that echoed
    /// the request would have shown the operator their own intent while the tone sat
    /// clamped at a band edge. `None` means no frequency-bearing module is live, which
    /// is the state a UI renders as a blank readout rather than as zero.
    pub frequency_hz: Option<f64>,
    /// Last-measured composite level, gRMS (random), when measured.
    pub measured_grms: Option<f64>,
    /// A soft alarm-band breach is active (not a stop).
    pub alarm_active: bool,
    /// The abort path fired.
    pub abort_active: bool,
    /// Seconds elapsed at the current level / in the current run phase.
    pub elapsed_s: Option<f64>,
    /// Seconds remaining in the current phase, when the schedule defines one.
    pub remaining_s: Option<f64>,
    /// Converging intents still in flight (see [`ActiveIntent`]).
    pub active_intents: Vec<ActiveIntent>,
    /// Target composite level at the CURRENT rung, gRMS (closed-loop random).
    /// ADDITIVE in v3, same rule as `module_state`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_grms: Option<f64>,
    /// Band coherence from the last sysID/gate (closed-loop random). ADDITIVE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub band_coherence: Option<f64>,
    /// The live module's OWN fine-grained state, verbatim (e.g. Random's
    /// `"engaged"` / `"full_level"` / `"coherence_hold"`), when a module is
    /// configured. The coarse [`RunState`] above stays the cross-module
    /// lifecycle; this is the string a module UI renders directly - mapping the
    /// coarse state back to a fine one guessed wrong on the very first remote
    /// run (FULL LEVEL shown while engaged at -12 dB, 2026-08-01). ADDITIVE
    /// within v3: absent on older peers, ignored by them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_state: Option<String>,
    /// Who is advancing the level schedule RIGHT NOW (closed-loop random):
    /// [`crate::LadderMode`], live-switchable mid-run via
    /// [`crate::Command::SetLadderMode`]. The panel renders THIS, not
    /// its own last request - after a takeover there must be no doubt who is
    /// driving. ADDITIVE within v3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ladder_mode: Option<crate::LadderMode>,
    /// The sample rate the run ASKED the station for, Hz
    /// ([`crate::AcquisitionProps::fs_hz`]). Carried BESIDE the actual, never
    /// instead of it: a single unattributed rate is what let the UI present a
    /// request as a measurement. ADDITIVE within v3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_fs_hz: Option<f64>,
    /// The sample rate the station actually PROGRAMMED, Hz - the adopted rate,
    /// not the request. A delta-sigma front end coerces the request up its own
    /// ladder (the NI 9234 rides 13.1072 MHz/(256n), so a requested 8192 is
    /// programmed as 8533.33), and every readout the operator indexes a profile
    /// by (`dF = fs/nPts`, block duration) is wrong by that ratio until this
    /// number crosses the seam.
    ///
    /// `None` is HONEST, not a gap: the station answers only at arm, so before
    /// that there is nothing to report and nothing may be invented. ADDITIVE
    /// within v3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_fs_hz: Option<f64>,
    /// Commanded drive amplitude, volts PEAK (open-loop modes) - v4.
    ///
    /// The open-loop counterpart of `level_db`: in a survey the operator commands
    /// VOLTS and measures response, so volts is the level readout and dB-relative-to-
    /// full has no referent. Carried as its own field rather than folded into
    /// `level_db` because a UI that mixed the two bases would eventually render a
    /// volts number with a dB label.
    ///
    /// This is what the core is COMMANDING, not what the amplifier delivered.
    /// `None` = no open-loop module is live. ADDITIVE (`serde(default,
    /// skip_serializing_if)`), the crate's established growth pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive_vpk: Option<f64>,
    /// Is a sweep running right now (open-loop modes) - v4.
    ///
    /// Three-valued on purpose: `Some(true)` sweeping, `Some(false)` a sweep-capable
    /// module is live and parked, `None` no such module is live. A plain `bool` would
    /// have folded "parked" and "not applicable" into one `false`, and the UI needs
    /// them apart - it suppresses spectrum averaging while sweeping (a moving tone
    /// must not smear) and must not suppress it for a module that never sweeps.
    /// ADDITIVE, same rule as `drive_vpk`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep_active: Option<bool>,
    /// Acquisition blocks the core's DISPLAY/analysis ring has dropped this run - v4.
    ///
    /// A drop-oldest ring keeps the display current by throwing the stale end away, which
    /// is the right trade and is also invisible: a display that has silently lost blocks
    /// looks exactly like one that has not. The rule here is to instrument every stage of a
    /// display path permanently rather than per-incident, and a counter that only reaches
    /// the core's log reaches nobody looking at the screen. Carried here so the front end's
    /// `streamProbe` can hold the PRODUCER-side loss beside the consumer-side counters it
    /// already keeps, and the whole path reads from one surface.
    ///
    /// Per-run total, monotonic within a run. `None` = the live module has no such ring
    /// (or nothing is configured), which is not the same as zero and must not render as
    /// it. ADDITIVE, same rule as `sweep_active`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_blocks_dropped: Option<u64>,

    // ---- v5: closed-loop SINE readouts ------------------------------------------
    /// Completed ONE-DIRECTION sweep passes this run (up + down = 2) - v5.
    ///
    /// The sweep progress an operator counts a test in. `None` = no sweeping module is
    /// live, which is not the same as "zero passes so far" and must not render as it.
    /// ADDITIVE, the crate's established growth pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweeps_done: Option<u32>,
    /// Total harmonic distortion of the control response, percent - v5.
    ///
    /// A DIAGNOSTIC, never an acceptance criterion: on a wire-jumper plant it reads
    /// the generator, and on a real article it is bounded by the amplifier and the
    /// fixture as much as by the control. Carried because an operator watching THD
    /// climb through a resonance learns something a level readout cannot tell them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thd_pct: Option<f64>,
    /// Fraction of the SCHEDULE completed, `[0, 1]` - v5.
    ///
    /// The module's own number, not `elapsed_s / (elapsed_s + remaining_s)` recomputed by
    /// a display. Both ends decide what counts as elapsed (a hold pauses test time; a
    /// level rung's dwell is not the sweep's), so a fraction derived at the far end is a
    /// second authority that agrees only until either end changes how it counts - and
    /// then disagrees silently, which is the worst way for two numbers to differ. The
    /// front end has `elapsed_s` and `remaining_s` already; what it does not have, and
    /// cannot honestly synthesise, is this.
    ///
    /// `None` = no module with a schedule is live, or the schedule has no determinable
    /// end (a free-running dwell). Renders as no progress bar at all, never as 0%.
    /// ADDITIVE, the crate's established growth pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    /// Why the run is HOLDING, verbatim from the module, when it is - v5.
    ///
    /// `None` = not holding. A hold is not a [`RunState`]: the run is still driving at
    /// the held frequency and level, and mapping it onto `Stopping` or `Armed` would
    /// tell a UI the output had stopped when it had not. So the coarse state stays
    /// what it is and this says why progress is not accruing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<String>,
    /// Is a limit notch currently BINDING the drive - v5.
    ///
    /// Three-valued for the same reason as `sweep_active`: `Some(true)` a ceiling is
    /// cutting the drive right now, `Some(false)` a notch-capable module is live and
    /// nothing is binding, `None` no such module is live. The distinction matters
    /// because a run that is under target BECAUSE a notch is holding it there is not a
    /// run that is failing to reach target, and an operator who cannot tell those apart
    /// will chase the wrong problem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notch_active: Option<bool>,
    /// The control target at the current frequency and rung, g PEAK - v6.
    ///
    /// The counterpart of [`Self::target_grms`] for the sine family, and a separate
    /// field rather than a reuse of it because the quantities are different: a gRMS is
    /// a broadband number over a band, a sine target is an amplitude at one frequency.
    /// A UI that read one into the other's label would print a peak with an RMS unit.
    ///
    /// This and [`Self::measured_g`] are THE closed-loop pair - target versus achieved
    /// is what says whether the loop is holding - and until v6 neither crossed, so a
    /// remote sine operator watched two permanently blank rows on the run rail while
    /// the daemon had both numbers in hand every control tick. `None` = no sine-family
    /// module is live. ADDITIVE, the crate's established growth pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_g: Option<f64>,
    /// The last-measured composite control amplitude, g PEAK - v6.
    ///
    /// The measured half of the pair described on [`Self::target_g`]. Composite means
    /// the control-set combination the run is configured for (weighted average, min or
    /// max), not one channel. `None` = not measured yet, or no sine-family module is
    /// live - which is not zero and must not render as it. ADDITIVE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured_g: Option<f64>,

    // ---- v5: SHOCK / SRS readouts ------------------------------------------------
    /// Pulses fired this run, total - v5. `None` = no shock-family module is live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pulses_fired: Option<u32>,
    /// Pulses fired at the CURRENT ladder level - v5.
    ///
    /// Carried beside the total rather than derived from it: the AUTO ladder's advance
    /// rule is per-level, so this is the number the schedule is actually counting, and
    /// a UI that subtracted its own running total would drift the moment a level jump
    /// reset the count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pulses_at_level: Option<u32>,
    /// Pulses still to fire in the train currently in flight - v5.
    ///
    /// `Some(0)` = no train in flight; `None` = no shock-family module is live. This
    /// advances DURING a train (it is written from inside the per-pulse fire path), so
    /// a mid-train status poll answers the truth rather than the pre-train plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub train_remaining: Option<u32>,
    /// Sequence number of the most recently fired pulse - v5.
    ///
    /// The handle [`crate::ShockCommand::PulseDetail`] fetches by. Carried as a plain
    /// counter beside `last_pulse` so a client can notice a NEW pulse without decoding
    /// the document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pulse_seq: Option<u64>,
    /// The most recently fired pulse's summary record - v5.
    ///
    /// A status-borne [`Doc`] at [`crate::SchemaKind::ShockPulse`], not a new `Event`
    /// variant. The trade, stated: a per-pulse event would be new wire machinery whose
    /// only advantage is push semantics the status path already has (`StateChanged`
    /// pushes on change), and it would be LOST across a reconnect - whereas a field on
    /// the status survives, so a controller that vanished and came back still sees what
    /// the last pulse did. `None` = nothing fired yet, or no shock-family module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pulse: Option<Doc>,
}

impl StatusReport {
    /// A blank idle report (no backend, nothing running, nothing claimed).
    pub fn idle() -> Self {
        StatusReport {
            backend: None,
            run: RunState::Idle,
            level_db: None,
            frequency_hz: None,
            measured_grms: None,
            alarm_active: false,
            abort_active: false,
            elapsed_s: None,
            remaining_s: None,
            active_intents: Vec::new(),
            target_grms: None,
            band_coherence: None,
            module_state: None,
            ladder_mode: None,
            requested_fs_hz: None,
            actual_fs_hz: None,
            drive_vpk: None,
            sweep_active: None,
            display_blocks_dropped: None,
            sweeps_done: None,
            thd_pct: None,
            progress: None,
            hold: None,
            notch_active: None,
            target_g: None,
            measured_g: None,
            pulses_fired: None,
            pulses_at_level: None,
            train_remaining: None,
            last_pulse_seq: None,
            last_pulse: None,
        }
    }
}

/// A core -> client response. Every command gets exactly one, with the command's
/// envelope `id` echoed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    /// The command was accepted; see [`Completion`] for whether it already happened
    /// or is converging.
    Accepted {
        /// Immediate vs converging.
        completion: Completion,
    },
    /// Answer to [`crate::Command::DescribeCapabilities`]. Boxed: the descriptor is
    /// by far the largest payload and would otherwise bloat every `Response`.
    Capabilities(Box<CapabilityDescriptor>),
    /// Answer to [`crate::Command::GetStatus`].
    ///
    /// **BOXED as of v5**, for the reason [`Response::Capabilities`] states: the
    /// descriptor used to be the largest payload here and the status report now is
    /// (v5 added nine readouts), so an unboxed variant makes every `Response` -
    /// including a bare `Accepted` - as big as a full status. `Box` is transparent to
    /// serde, so the WIRE SHAPE IS UNCHANGED and no peer sees a difference.
    Status(Box<StatusReport>),
    /// Answer to [`crate::Command::ListBackends`].
    Backends(Vec<BackendInfo>),
    /// Answer to [`crate::Command::GetChannelTable`] and to an ACCEPTED
    /// [`crate::Command::PutChannelTable`] (v3).
    ///
    /// A successful write answers with the table AS STORED and its NEW etag, rather
    /// than a bare acknowledgement. That is deliberate: the client needs the fresh
    /// etag to make a second edit, and answering with the stored document means a
    /// core-side migration or normalization is visible immediately instead of being
    /// discovered on the next read.
    ChannelTable(TableDoc),
    /// Answer to [`crate::Command::ListCaptures`] (v3). Metadata only - the capture
    /// bytes stay on the core host.
    Captures(Vec<CaptureInfo>),
    /// Answer to a read-only [`crate::RecorderCommand`] (v3), as an opaque versioned
    /// document. Mutating recorder verbs answer with
    /// [`Response::Accepted`] like every other mutating command.
    Recorder(Doc),
    /// Answer to a read-only [`crate::ShockCommand`] / [`crate::SrsCommand`] (v5):
    /// `PulseList`, `PulseDetail`, `SrsFreqs`. Mutating shock verbs answer with
    /// [`Response::Accepted`] like every other mutating command.
    ///
    /// The document is stamped [`crate::SchemaKind::ShockPulse`] or
    /// [`crate::SchemaKind::ShockSrsGrid`] by the core; the KIND is not carried on the
    /// [`Doc`] itself (see [`crate::SchemaKind`]'s docs), so a client reads the answer
    /// it asked for and never routes on a kind field.
    ///
    /// **The one-directional additivity invariant applies:** this variant may only be
    /// emitted in reply to a command an OLD peer cannot send. An old peer never sends
    /// a shock verb - the version gate refused it at connect - so it can never receive
    /// a response shape it cannot decode.
    Shock(Doc),
    /// Answer to a read-only [`crate::TwrCommand`] (v6) - `ReferenceInfo`, `History` -
    /// and to an accepted [`crate::TwrCommand::CommitReference`]. Every other TWR verb
    /// answers [`Response::Accepted`] like the rest of the wire.
    ///
    /// **The commit answers a DOCUMENT rather than a bare acceptance**, which is the
    /// [`Response::ChannelTable`] precedent applied to the upload: a client that just
    /// staged a megabyte needs to know WHAT was staged - the sha the core actually
    /// computed over the bytes it holds, the sample count it counted, and the rate it
    /// will play them at. Answering "accepted" would leave the client believing its own
    /// declaration, which is exactly the thing the commit exists to check.
    ///
    /// Stamped [`crate::SchemaKind::TwrReference`] by the core; the kind is NOT carried
    /// on the [`Doc`] (see [`crate::SchemaKind`]'s docs), so a client reads the answer
    /// to the query it asked and never routes on a kind field.
    ///
    /// **The one-directional additivity invariant applies:** this variant may only be
    /// emitted in reply to a command an OLD peer cannot send. An old peer never sends a
    /// TWR verb - the version gate refused it at connect - so it can never receive a
    /// response shape it cannot decode.
    Twr(Doc),
    /// The command was refused; the error says why (and crosses the wire verbatim).
    Rejected {
        /// Why.
        error: ProtocolError,
    },
}
