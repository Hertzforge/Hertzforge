//! Core -> client events: the unsolicited stream (frames, state, convergence).
//!
//! # How converging intents are modeled
//!
//! On device-paced hardware an intent like "go to 500 Hz" is SECONDS of paced work:
//! the core-side delegate re-measures, nudges, clamps at safety edges, and may stop
//! early. The vocabulary here is taken from the real converger (the explorer pacer's
//! jump result): progress with an honest basis (measured vs open-loop estimate), and
//! a settled outcome that distinguishes converged / clamped-at-a-band-edge / budget
//! exhausted / superseded by a newer intent / aborted / failed. A UI can therefore
//! show live progress, and an operator retargeting mid-flight (turn the dial again)
//! sees the old intent settle as [`IntentOutcome::Superseded`] instead of an error.

use serde::{Deserialize, Serialize};

use crate::caps::CapabilityDescriptor;
use crate::docs::Doc;
use crate::error::ProtocolError;
use crate::frames::{FrameBytes, StreamKind};
use crate::response::{IntentId, StatusReport};
use crate::session::{LeaseAnswer, RunRole};

/// Whether a progress readout is a real measurement or an open-loop estimate.
/// Honest by design: on paced hardware the drive level is NOT telemetered, so its
/// convergence progress is an estimate with growing uncertainty, and the UI should
/// present it as such (frequency, by contrast, is re-measured every tick).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// Re-measured from telemetry this tick.
    Measured,
    /// Advanced open-loop from a step model; no measurement exists.
    Estimated,
}

/// Engineering unit of a progress readout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineeringUnit {
    /// Hertz.
    Hertz,
    /// Decibels (relative to the profile's full level).
    Decibels,
    /// Volts at the drive output.
    Volts,
    /// Composite gRMS.
    GRms,
}

/// One convergence progress sample.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// Where the quantity is now (in `unit`).
    pub current: f64,
    /// Where the intent is converging to (in `unit`).
    pub target: f64,
    /// The unit of `current` / `target`.
    pub unit: EngineeringUnit,
    /// Measurement vs estimate (see [`Basis`]).
    pub basis: Basis,
    /// Optional normalized progress in `[0, 1]` for progress bars, when the core can
    /// compute something meaningful (`None` otherwise - never fabricated).
    pub fraction: Option<f32>,
}

/// How a converging intent ended. The non-`Completed` outcomes are NORMAL vocabulary
/// on paced hardware, not exotic errors - UIs must render them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentOutcome {
    /// Converged within the deadband of the target.
    Completed,
    /// Stopped early at a safety edge (operator band edge, drive ceiling) and holds
    /// there; the run continues. Partial success, not an error.
    Clamped {
        /// Where it holds (in the intent's unit).
        at: f64,
        /// Which edge, human-readable.
        reason: String,
    },
    /// The convergence budget (e.g. a bounded nudge count guarding against a bad
    /// calibration) ran out before the deadband; holds where it got to.
    Incomplete {
        /// Where it stopped (in the intent's unit).
        at: f64,
        /// What bound was hit, human-readable.
        reason: String,
    },
    /// A newer intent retargeted the same quantity; that intent carries on from
    /// here. Expected whenever the operator adjusts mid-convergence.
    Superseded,
    /// A stop/abort ended the run (and with it this intent).
    Aborted,
    /// The intent failed (link refused, disconnect, ...); the error says why.
    Failed {
        /// Why.
        error: ProtocolError,
    },
}

/// A core -> client event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    /// A converging intent advanced (see the module docs).
    IntentProgress {
        /// Which intent.
        intent: IntentId,
        /// The sample.
        progress: Progress,
    },
    /// A converging intent reached a terminal state; nothing further arrives for it.
    IntentSettled {
        /// Which intent.
        intent: IntentId,
        /// How it ended.
        outcome: IntentOutcome,
    },
    /// One packed display frame, carried opaque and bit-for-bit (see
    /// [`crate::frames`]). The decimated display path is the ONLY data path that
    /// crosses this seam - never the control loop.
    Frame {
        /// Which pinned layout `bytes` holds.
        stream: StreamKind,
        /// The packed frame, exactly as the producer emitted it.
        bytes: FrameBytes,
    },
    /// The run status changed (state transition, alarm, abort, level/frequency
    /// milestone). Also available by poll via [`crate::Command::GetStatus`].
    StateChanged {
        /// The fresh report. **Boxed as of v5**, for the same reason as
        /// [`Event::CapabilitiesChanged`]'s `caps`: v5 added nine readouts and the
        /// status report is now the largest payload this enum carries, so an unboxed
        /// field makes every `Event` - including a display `Frame`, the one that
        /// arrives tens of times a second - as big as a full status. `Box` is
        /// transparent to serde, so the WIRE SHAPE IS UNCHANGED.
        status: Box<StatusReport>,
    },
    /// The active backend's capabilities changed (station switch, link
    /// gained/lost, a posture learned at runtime). The UI must re-adapt; cached
    /// capability state is stale after this.
    CapabilitiesChanged {
        /// The fresh descriptor. Boxed for the same reason as
        /// [`crate::Response::Capabilities`].
        caps: Box<CapabilityDescriptor>,
    },
    /// A run STARTED, carrying its provenance block (v3): the station class, the
    /// devices, whether the driver says they are simulated, the core build, the run
    /// id.
    ///
    /// This is the remote half of the answer to "what IS this data". It is an event
    /// rather than a field on the status report because it is a fact about a run, not
    /// a level that changes - and because a remote UI that attaches mid-run needs to
    /// receive it on connect, which an event stream does naturally.
    ///
    /// Opaque [`Doc`] at [`crate::PROVENANCE_SCHEMA`]: the provenance block's owning
    /// type lives on the hardware side (see [`crate::docs`]).
    RunProvenance {
        /// The provenance block, verbatim JSON.
        provenance: Doc,
    },
    /// This connection's run authority changed (v3) - the Controller vanished and the
    /// lease became free, or this Observer was promoted, or this Controller's lease
    /// was released.
    ///
    /// A UI must re-render its controls from this. It is the only way an Observer
    /// learns that the lease is available without polling, and the only way a
    /// reconnecting Controller learns it did NOT get its old lease back.
    LeaseChanged {
        /// This connection's role now.
        role: RunRole,
        /// The full answer, so the UI can name the holder and distinguish a
        /// downgrade from a plain observer session.
        lease: LeaseAnswer,
    },
    /// The core is bringing the run down for a reason the operator did not ask for
    /// (v3) - today, the CONTROLLER TIMEOUT: the peer holding the lease stopped
    /// heartbeating and did not come back.
    ///
    /// It is a distinct event rather than a `StateChanged` with a note because the
    /// two say different things. `StateChanged` reports that the state IS `Stopping`;
    /// this reports WHY, and it is emitted to every connected peer, including the
    /// Observers who are still watching and who would otherwise see a run end for no
    /// visible reason.
    ///
    /// The stop is always the CONTROLLED ramp-down path. "Nobody is watching" is a
    /// reason to stop safely, never a reason to slam the drive to zero.
    SafeStopping {
        /// Human-readable cause, ready to show.
        reason: String,
    },
}
