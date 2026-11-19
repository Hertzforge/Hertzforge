//! The protocol error type - serializable, so a refusal crosses the wire verbatim.
//!
//! Hand-rolled `Display` + `std::error::Error` (no `thiserror`), matching the
//! workspace idiom in `hertzforge-hal` and keeping this crate's dependency floor at serde
//! alone (see the crate docs: the floor is load-bearing).

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::caps::{ActionKind, TestMode};
use crate::docs::SchemaKind;

/// Everything that can go wrong at this seam, in a shape both peers can serialize.
///
/// The vocabulary deliberately mirrors the HAL's error posture (`NotConnected`,
/// unsupported-with-a-reason, and a first-class disconnect) without depending on the
/// HAL crate, so a backend-side failure can be carried to a remote UI verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolError {
    /// The peers speak different protocol versions. This error is FINAL for the
    /// connection: the correct response is to surface both numbers to the human and
    /// refuse to continue, never to guess at a foreign wire layout. See
    /// [`crate::envelope`] for the compatibility rule.
    VersionMismatch {
        /// The version this peer speaks ([`crate::PROTOCOL_VERSION`] at build time).
        ours: u16,
        /// The version the remote peer declared on the wire.
        theirs: u16,
    },
    /// The active backend cannot perform this action at all (its
    /// [`crate::ActionMatrix`] entry is [`crate::ActionSupport::Unsupported`]).
    UnsupportedAction {
        /// The refused action.
        action: ActionKind,
    },
    /// The active backend cannot run this test mode (its [`crate::ModeMatrix`] entry
    /// is [`crate::ModeSupport::Unsupported`]); mirrors the rule
    /// that `arm()` HARD-REFUSES an unsupported (module x backend) pair.
    UnsupportedMode {
        /// The refused mode.
        mode: TestMode,
    },
    /// The hardware link was up and has been lost (mirrors the HAL's disconnected
    /// posture). Distinct from [`ProtocolError::NotConnected`]: this one means a run
    /// may have been interrupted.
    Disconnected,
    /// No hardware link is open; the command needs one.
    NotConnected,
    /// The command was understood but its arguments are invalid for the current
    /// configuration (bad profile, level outside the schedule, frequency outside the
    /// band, ...).
    InvalidArgument {
        /// Human-readable reason.
        message: String,
    },
    /// The core refused the command in its current state (already running, link
    /// posture cannot encode the requested direction, planned-but-not-installed
    /// backend, ...). The message says why.
    Refused {
        /// Human-readable reason.
        message: String,
    },
    /// An internal core-side failure the UI can only report.
    Internal {
        /// Human-readable reason.
        message: String,
    },
    /// The handshake did not present an acceptable shared token (v3). FINAL for the
    /// connection: the core closes it after sending this.
    Unauthorized {
        /// Which of the three failure shapes it was (wrong / absent / the core is
        /// configured with a blank token). Never echoes the offered token.
        detail: String,
    },
    /// A mutating command arrived from an OBSERVER (v3). The run lease is held by
    /// someone else - or by nobody, if the Controller has vanished and the lease has
    /// not yet timed out.
    ///
    /// A UI receiving this has rendered a control it should not have: the
    /// [`crate::HelloAck`] said which role this peer holds, before any control was
    /// drawn. Treat it as a bug report about the UI, not as a prompt to retry.
    NotController {
        /// Who holds the lease, for the message. Empty when nobody does.
        holder: String,
    },
    /// A channel-table write quoted an etag that is no longer current (v3): someone
    /// else wrote the table in between. Re-read, re-apply, re-send. NEVER resolved by
    /// retrying with the new etag automatically - the point of the refusal is that a
    /// human sees what changed, because the document carries abort limits.
    StaleUpload {
        /// The etag the core holds now.
        current_etag: String,
        /// The etag the write quoted.
        provided_etag: String,
    },
    /// A versioned document was written at a schema this build does not speak (v3).
    /// Exact match, same posture as [`ProtocolError::VersionMismatch`] one layer up -
    /// see [`crate::docs`].
    UnsupportedSchema {
        /// Which document.
        kind: SchemaKind,
        /// The schema this peer speaks.
        ours: u32,
        /// The schema the document declared.
        theirs: u32,
    },
    /// A payload could not be decoded (v3). `detail` carries the DECODER'S OWN
    /// message verbatim, which is what makes this the unknown-field diagnostic: a
    /// profile carrying a field this build does not know fails
    /// `deny_unknown_fields` with "unknown field `x`, expected one of ...", and that
    /// sentence is what reaches the operator instead of a run that quietly ignored
    /// part of the profile.
    MalformedPayload {
        /// What was being decoded (`"command"`, `"profile"`, `"channel table"`, ...).
        container: String,
        /// The decoder's message, verbatim.
        detail: String,
    },
    /// The command is real, and this core does not serve it over THIS seam (v3).
    ///
    /// The remote-mode honesty verb. The v3 daemon covers the Recorder; every other
    /// module runs only in a local Tauri app, and a remote UI must render that as an
    /// explicit "local-only in remote mode" state. The alternative - falling back to
    /// a local backend when the remote one has no answer - would put a run on
    /// different hardware than the operator believes, which is the same class of
    /// defect as a Sim fallback.
    LocalOnly {
        /// What is not served here, named for the UI (`"Sine module"`, ...).
        what: String,
    },
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::VersionMismatch { ours, theirs } => write!(
                f,
                "protocol version mismatch: this peer speaks v{ours}, the remote peer \
                 speaks v{theirs}; refusing to continue (upgrade the older peer)"
            ),
            ProtocolError::UnsupportedAction { action } => {
                write!(f, "action not supported by the active backend: {action:?}")
            }
            ProtocolError::UnsupportedMode { mode } => {
                write!(f, "test mode not supported by the active backend: {mode:?}")
            }
            ProtocolError::Disconnected => write!(f, "hardware link lost"),
            ProtocolError::NotConnected => write!(f, "no hardware link is open"),
            ProtocolError::InvalidArgument { message } => write!(f, "invalid argument: {message}"),
            ProtocolError::Refused { message } => write!(f, "refused: {message}"),
            ProtocolError::Internal { message } => write!(f, "internal error: {message}"),
            ProtocolError::Unauthorized { detail } => {
                write!(f, "connection refused at the handshake: {detail}")
            }
            ProtocolError::NotController { holder } => write!(
                f,
                "refused: this connection is an OBSERVER and may not change core \
                 state; the run lease is held by {}. Read-only clients see status, \
                 frames and lists; the physical E-stop is the answer for anyone \
                 without the lease",
                if holder.is_empty() {
                    "nobody (the previous controller has not timed out yet)"
                } else {
                    holder
                }
            ),
            ProtocolError::StaleUpload {
                current_etag,
                provided_etag,
            } => write!(
                f,
                "refused a STALE channel-table write: it quotes etag {provided_etag} \
                 and the core now holds {current_etag}, so someone else wrote the \
                 table in between. Re-read the table, re-apply the edit, and send it \
                 again - this document carries the abort limits, so last-write-wins \
                 is not an option"
            ),
            ProtocolError::UnsupportedSchema { kind, ours, theirs } => write!(
                f,
                "document schema mismatch ({kind:?}): this peer speaks v{ours}, the \
                 document declares v{theirs}; refusing to interpret it"
            ),
            ProtocolError::MalformedPayload { container, detail } => {
                write!(f, "malformed {container}: {detail}")
            }
            ProtocolError::LocalOnly { what } => write!(
                f,
                "{what} is LOCAL-ONLY in remote mode: this core does not serve it \
                 over the network seam. Run it in the local application on the \
                 hardware host. There is no fallback - serving it from the front end \
                 would put the run on different hardware than the operator selected"
            ),
        }
    }
}

impl std::error::Error for ProtocolError {}
