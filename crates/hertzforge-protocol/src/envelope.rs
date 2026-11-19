//! The versioned on-wire envelope and the handshake.
//!
//! # The compatibility rule (v1)
//!
//! [`PROTOCOL_VERSION`] is the single protocol version constant, and every envelope
//! carries it in its leading `v` field. The v1 policy is EXACT MATCH:
//!
//! - Peers exchange a [`Hello`] before any command traffic and call [`Hello::check`];
//!   a mismatch fails the connection at establishment time with
//!   [`ProtocolError::VersionMismatch`] naming both versions, so the human knows
//!   which peer to upgrade.
//! - Every envelope is checked again at dispatch ([`Envelope::check_version`], run
//!   for you by [`crate::dispatch`]), as defense in depth against a peer that skipped
//!   the handshake or a stream that got crossed. A mismatched command NEVER reaches
//!   the core.
//!
//! Incompatible peers therefore fail LOUDLY and EARLY. There is deliberately no
//! best-effort parse of a foreign version: this seam carries operator commands aimed
//! at physical shakers, and a silently misparsed field (a level, a frequency) is a
//! materially worse failure than a refused connection. Any future negotiate-down
//! scheme must keep that property: a peer that does not positively understand the
//! remote version refuses it.
//!
//! # The BUILD STAMP is compared and reported, never enforced
//!
//! [`Hello`] carries a [`CoreStamp`] (git sha + dirty flag + host + target) alongside
//! the version. It is deliberately NOT part of the compatibility gate: two dev
//! machines building the same protocol version legitimately differ in all four
//! fields, and the split deployment is a Mac front end talking to a Linux core -
//! refusing on a stamp mismatch would break the intended topology on day one.
//!
//! So the rule has two halves, and both matter:
//!
//! - **The protocol version stays EXACT-MATCH.** That is what protects the operator.
//! - **The stamp is COMPARED and REPORTED.** [`Hello::compare_stamp`] answers
//!   match / differs / unknown, and a differing stamp is a logged WARNING plus a
//!   surfaced field. It exists so that "I fixed it and the symptom did not change"
//!   can be answered with evidence instead of a guess.
//!
//! # The handshake carries AUTHORITY and a TOKEN too (v3)
//!
//! v3 makes the handshake do three jobs instead of one, because all three have to be
//! settled before the first command and none of them can be renegotiated later:
//!
//! 1. **Version** - unchanged, exact match, the gate.
//! 2. **Admission** - a shared [`Hello::token`], checked here. A hardware host that
//!    accepts any TCP connection on the lab LAN is one misconfiguration away from a
//!    stranger's `SetLevel`. The token is not a security system (it is a shared
//!    secret over a plaintext socket on a trusted network); it is the door that stops
//!    an accidental connection from a second workstation.
//! 3. **Run authority** - which peer holds the run lease. See [`crate::session`] for
//!    the one-Controller rule and the deliberate REFUSAL-by-default answer to a
//!    second Controller.
//!
//! The answer to all three is one [`HelloAck`], so a client knows what it is before
//! it renders a single control.
//!
//! Version history:
//! - **v1** - the initial seam.
//! - **v2** (wire bump #1) - the two-axis
//!   [`crate::ModeSupport`] (`DriveOrigin` x `AdaptationOwner` + a typed refusal
//!   reason), the binary frame headers' observability block
//!   (`seq`/`sample_index`/`run_id`, see `ui/IPC_CONTRACT.md`), `MCS1` documented as
//!   the universal spectrum layout, and the [`CoreStamp`] on this handshake. Batched
//!   into ONE bump so the wire changes once. See `PROTOCOL_CHANGELOG.md`.
//! - **v3** (wire bump #2) - [`crate::StreamSub`] selectors on `Subscribe`;
//!   `schema_version` on the profile payloads and on the new opaque documents
//!   ([`crate::Doc`] / [`crate::TableDoc`]); the run lease + shared token on this
//!   handshake plus the [`HelloAck`] that answers it; the capture LIST verb; the
//!   channel-table get/put verbs with etag concurrency; the Recorder command group.
//! - **v4** (wire bump #3, Explorer-on-NI) - the
//!   [`crate::TestProfile::Explorer`] profile variant, the
//!   [`crate::ExplorerCommand`] group (`StartSweep` / `StopSweep`), and three
//!   [`crate::StatusReport`] additions (`drive_vpk`, `sweep_active`, plus FILLED
//!   semantics for the pre-existing `frequency_hz`). Batched into ONE bump for the
//!   same reason as v2 and v3: with an exact-match gate there is no graceful
//!   degradation, so every wire change lands before the deployment that would
//!   otherwise have to negotiate it.
//!
//!   **`SineProfile` was deliberately NOT grown into.** It is
//!   `deny_unknown_fields`, so a new field on it is not additive - an older peer
//!   handed one fails with a serde field error in the middle of a workflow, on the
//!   payload that points a shaker. A NEW variant costs one version bump and cannot
//!   mis-decode on any peer that got past the gate.
//! - **v5** (wire bump #4, Sine-on-NI + Shock-on-NI) - THREE modules' surfaces
//!   and one shared routing bundle, jointly authored by the two sessions building them
//!   and landed in ONE increment. [`crate::TestProfile`] gains `Sine`, `ShockClassical`
//!   and `Srs`; [`Command`] gains the [`crate::SineCommand`], [`crate::ShockCommand`]
//!   and [`crate::SrsCommand`] groups; [`crate::StatusReport`] gains nine additive
//!   readouts; [`crate::Response`] gains `Shock(Doc)`; [`crate::SchemaKind`] gains the
//!   two shock document kinds. [`crate::PROFILE_SCHEMA`] deliberately STAYS 1.
//!
//!   **Why one bump for three modules.** Two sessions were building against this seam
//!   at the same time. Three bumps would have meant three coordinated daemon-then-app
//!   redeploys, each with its own connect-refusal window, and the middle two would have
//!   been refusing peers over surfaces nobody was using yet. The exact-match rule makes
//!   batching the cheap option and shipping-as-you-go the expensive one, which is the
//!   same argument v2, v3 and v4 each made.
//!
//!   **The wire ACCEPTS shapes the core does not yet serve, and says so.** All three
//!   new profile variants and all three new command groups decode cleanly and are then
//!   answered with a TYPED refusal naming what is missing, until the matching daemon
//!   host lands. Never an acknowledgement, never a silent drop: a shape that parsed and
//!   went nowhere is the failure this seam exists to prevent.
//! - **v6** (wire bump #5, TWR-on-NI) - ONE module's surface, and the first bump
//!   whose payload does not fit in a profile. [`crate::TestProfile`] gains
//!   `Twr(Box<TwrProfile>)`; [`Command`] gains the [`crate::TwrCommand`] group, whose
//!   eleven verbs include the chunked REFERENCE UPLOAD (`BeginReference` ->
//!   `ReferenceChunk` -> `CommitReference`); [`crate::Response`] gains `Twr(Doc)`;
//!   [`crate::SchemaKind`] gains `TwrReference`; and [`crate::TestMode`] gains its
//!   FIRST new variant since this crate existed, with `ModeMatrix` growing the matching
//!   row. [`crate::PROFILE_SCHEMA`] deliberately STAYS 1, for the third bump running.
//!
//!   **One module, not three, and the batching argument still holds.** v5 batched
//!   because two sessions were building at once; v6 does not batch because TWR was the
//!   only surface waiting when the bump was taken. The rule was never "collect
//!   modules" - it is "move the wire as few times as possible", and holding a finished
//!   surface back to wait for a hypothetical second one would trade a certain delay for
//!   a speculative saving.
//!
//!   **`TestMode` gains a variant whose `hertzforge_hal::NativeMode` mirror does NOT.** TWR
//!   is host-only on every path - its law runs between plays even under delegation - so
//!   there is no native mode for a device to prove. `caps.rs`'s module docs carry the
//!   exception explicitly, and the consequence is that `modes.twr` can only ever be
//!   claimed by a manual assignment citing a bench ledger.
//!
//!   **SERVED NOW: nothing.** Every TWR verb and the profile arm answer the typed
//!   MISSING-HOST refusal until the daemon's `TwrHost` lands. Same posture as v5's
//!   three surfaces on the day they shipped, and same promise: the refusal becomes an
//!   acceptance with no further wire change.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::error::ProtocolError;
use crate::session::{LeaseAnswer, RunRole};
use crate::{Command, Event, Response};

/// The protocol version this build speaks. Bump on ANY change to the wire shape of
/// [`Command`], [`Response`], [`Event`], the envelope itself, the binary frame
/// headers, or the meaning of an existing field. Exact match is required end-to-end
/// (see the module docs).
pub const PROTOCOL_VERSION: u16 = 6;

/// The peer's CORE build identity, mirrored from `hertzforge_buildinfo::BuildInfo`.
///
/// Mirrored, not imported: this crate's dependency floor is serde and nothing else
/// (see the crate docs' "Dependency discipline"), and the same rule that keeps
/// `hertzforge-hal` out keeps `hertzforge-buildinfo` out. The server side constructs this from
/// `hertzforge_buildinfo::build_info()`; the field names and the `"unknown"` sentinel are
/// kept aligned by review, exactly like the `Caps` mirror.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreStamp {
    /// Short git SHA, or `"unknown"` outside a git tree.
    pub sha: String,
    /// Uncommitted tracked changes at build time.
    pub dirty: bool,
    /// Build host, or `"unknown"`.
    pub host: String,
    /// Target triple, or `"unknown"`.
    pub target: String,
}

impl CoreStamp {
    /// A stamp that claims nothing - what a peer sends when it has no build info to
    /// report. Distinguishable from every real stamp, and never reported as a
    /// mismatch (see [`StampComparison::Unknown`]).
    pub fn unknown() -> Self {
        CoreStamp {
            sha: "unknown".into(),
            dirty: false,
            host: "unknown".into(),
            target: "unknown".into(),
        }
    }

    /// True when this stamp carries no usable identity.
    pub fn is_unknown(&self) -> bool {
        self.sha == "unknown"
    }

    /// Compare two stamps. **The result is REPORTED, never used to refuse a
    /// connection** - see the module docs.
    pub fn compare(&self, other: &CoreStamp) -> StampComparison {
        if self.is_unknown() || other.is_unknown() {
            StampComparison::Unknown
        } else if self.sha == other.sha && self.dirty == other.dirty {
            StampComparison::Match
        } else {
            StampComparison::Differs
        }
    }
}

impl fmt::Display for CoreStamp {
    /// `a1b2c3d-dirty @ linux-bench (x86_64-unknown-linux-gnu)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.sha)?;
        if self.dirty {
            write!(f, "-dirty")?;
        }
        write!(f, " @ {} ({})", self.host, self.target)
    }
}

/// The outcome of comparing two [`CoreStamp`]s.
///
/// Three-valued deliberately. A two-valued match/mismatch would force the unknowable
/// case (a build with no git tree) to report as a mismatch, indistinguishable from a
/// real one - and a diagnostic that cannot say "I do not know" is a diagnostic that
/// will eventually be believed when it should not be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StampComparison {
    /// Same commit and same dirty state. Host and target may still differ - normal
    /// for a Mac-UI / Linux-core split, and NOT a mismatch.
    Match,
    /// Different commit, or the same commit with one side dirty. WARN and surface;
    /// do NOT refuse.
    Differs,
    /// At least one side has no stamp to compare. Say so; claim nothing.
    Unknown,
}

impl fmt::Display for StampComparison {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StampComparison::Match => f.write_str("match"),
            StampComparison::Differs => f.write_str("differs"),
            StampComparison::Unknown => f.write_str("unknown"),
        }
    }
}

/// Check a remote version against [`PROTOCOL_VERSION`]; the single comparison every
/// gate in this crate uses.
pub fn check_version(theirs: u16) -> Result<(), ProtocolError> {
    if theirs == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(ProtocolError::VersionMismatch {
            ours: PROTOCOL_VERSION,
            theirs,
        })
    }
}

/// The on-wire envelope: version, correlation id, payload. One shape for all three
/// payload kinds ([`CommandEnvelope`], [`ResponseEnvelope`], [`EventEnvelope`]).
///
/// `id` semantics by payload kind:
/// - Command: chosen by the client, unique per in-flight request.
/// - Response: echoes the command's `id` (that is the request/response correlation).
/// - Event: a server-assigned monotonic sequence number per connection, so a UI can
///   detect gaps after a reconnect. (Convergence correlation does NOT ride this
///   field - it rides the [`crate::IntentId`] inside the event payload.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    /// On-wire protocol version; must equal [`PROTOCOL_VERSION`] (see module docs).
    /// First field on purpose, so version-first decoding (see [`VersionHeader`])
    /// works even in field-order binary codecs.
    pub v: u16,
    /// Correlation id (see the type docs for per-kind semantics).
    pub id: u64,
    /// The payload.
    pub body: T,
}

impl<T> Envelope<T> {
    /// Wrap a payload, stamping this build's [`PROTOCOL_VERSION`].
    pub fn new(id: u64, body: T) -> Self {
        Envelope {
            v: PROTOCOL_VERSION,
            id,
            body,
        }
    }

    /// Enforce the exact-match version rule on a decoded envelope. [`crate::dispatch`]
    /// calls this before the payload ever reaches the core.
    pub fn check_version(&self) -> Result<(), ProtocolError> {
        check_version(self.v)
    }
}

/// A command envelope (client -> core).
pub type CommandEnvelope = Envelope<Command>;
/// A response envelope (core -> client, one per command, `id` echoed).
pub type ResponseEnvelope = Envelope<Response>;
/// An event envelope (core -> client, unsolicited, `id` = monotonic sequence).
pub type EventEnvelope = Envelope<Event>;

/// A version-only probe view of any envelope or [`Hello`]: decode THIS first when you
/// want to reject a foreign version before touching the rest of the payload
/// (self-describing codecs ignore the trailing fields; field-order codecs can decode
/// the leading `v` because it is the first field everywhere in this crate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionHeader {
    /// The remote peer's declared protocol version.
    pub v: u16,
}

/// Which side of the seam a peer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerRole {
    /// The front end (display + operator input).
    Ui,
    /// The core (hardware host).
    Core,
}

/// The handshake message. Each peer sends one before any other traffic and checks the
/// one it receives; see the module docs for the rule this enforces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    /// The sender's protocol version. First field (see [`VersionHeader`]).
    pub v: u16,
    /// The sender's role.
    pub role: PeerRole,
    /// Free-form build tag for diagnostics (app name / version). Never parsed, only
    /// shown to humans in mismatch reports and logs.
    pub build: String,
    /// The sender's CORE build stamp (v2). **Compared and reported, NEVER enforced** -
    /// see the module docs and [`Hello::compare_stamp`].
    ///
    /// `#[serde(default)]` so the field is additive for any encoder that omits it; the
    /// default is [`CoreStamp::unknown`], which reports as
    /// [`StampComparison::Unknown`] rather than as a mismatch.
    #[serde(default = "CoreStamp::unknown")]
    pub stamp: CoreStamp,
    /// The run authority this peer is ASKING for (v3). Defaults to
    /// [`RunRole::Observer`] - the least authority - so a client that omits the field
    /// cannot acquire the right to drive by accident. See [`crate::session`].
    #[serde(default)]
    pub run_role: RunRole,
    /// May the core answer a Controller request with an Observer session instead of
    /// refusing the connection (v3)?
    ///
    /// **Defaults to `false`, and that default is the whole design.** A UI that
    /// believes it is in control and is silently not will render enabled Start / Stop
    /// / Abort controls that do nothing. Refusing the connection with a message
    /// naming the current holder is strictly better. An observer-first client opts in
    /// deliberately; see [`crate::session`] for the full argument.
    #[serde(default)]
    pub accept_observer_downgrade: bool,
    /// The shared admission token (v3), or `None` when the peer has none to offer.
    ///
    /// Checked at the handshake, before any command traffic, and answered with
    /// [`ProtocolError::Unauthorized`]. Not a security system - a shared secret over
    /// a plaintext socket - but the door that stops an accidental connection from a
    /// second workstation on the same lab LAN from reaching a shaker.
    #[serde(default)]
    pub token: Option<String>,
}

impl Hello {
    /// Build a handshake for this peer, stamping [`PROTOCOL_VERSION`], with NO core
    /// stamp. Use [`Hello::with_stamp`] wherever a stamp is available; a peer that
    /// omits it is honestly reporting that it does not know its own build.
    pub fn new(role: PeerRole, build: impl Into<String>) -> Self {
        Hello {
            v: PROTOCOL_VERSION,
            role,
            build: build.into(),
            stamp: CoreStamp::unknown(),
            run_role: RunRole::default(),
            accept_observer_downgrade: false,
            token: None,
        }
    }

    /// Build a handshake carrying this peer's core build stamp.
    pub fn with_stamp(role: PeerRole, build: impl Into<String>, stamp: CoreStamp) -> Self {
        Hello {
            stamp,
            ..Hello::new(role, build)
        }
    }

    /// Ask for a run role (v3). Chainable with [`Hello::with_token`] and
    /// [`Hello::accepting_observer_downgrade`].
    pub fn asking(mut self, run_role: RunRole) -> Self {
        self.run_role = run_role;
        self
    }

    /// Offer the shared admission token (v3).
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// Opt in to being handed an Observer session instead of being refused, when a
    /// Controller already holds the lease (v3). See [`crate::session`] for why this
    /// is opt-in rather than the default.
    pub fn accepting_observer_downgrade(mut self) -> Self {
        self.accept_observer_downgrade = true;
        self
    }

    /// Enforce the exact-match version rule on a received handshake. On `Err`, close
    /// the connection after surfacing the error - do not fall back to command traffic.
    ///
    /// **The build stamp is NOT consulted here.** That is the design, not an
    /// oversight: see [`Hello::compare_stamp`].
    pub fn check(&self) -> Result<(), ProtocolError> {
        check_version(self.v)
    }

    /// Compare the remote's core stamp against ours. The result is for LOGGING and
    /// for a surfaced UI field; a [`StampComparison::Differs`] must not refuse the
    /// connection, because a Mac front end and a Linux core built from the same commit
    /// legitimately differ in host and target, and two dev machines legitimately
    /// differ in sha.
    ///
    /// Suggested handling: `Match` - nothing; `Differs` -
    /// a WARNING naming both stamps plus a field the UI can show; `Unknown` - a field
    /// saying so, and no claim either way.
    pub fn compare_stamp(&self, ours: &CoreStamp) -> StampComparison {
        ours.compare(&self.stamp)
    }

    /// Enforce the shared admission token (v3).
    ///
    /// `expected` is what the core was configured with. An EMPTY `expected` refuses
    /// every peer rather than authorizing all of them: a blank token in a config file
    /// is a mistake, and the failure mode of reading it as "no authentication
    /// required" is a shaker on an open port.
    ///
    /// The comparison is a plain equality. This is a shared secret over a plaintext
    /// socket on a trusted lab network, and dressing it in constant-time compare
    /// would imply a threat model this transport does not have - if the adversary can
    /// time the comparison they can also read the token off the wire.
    pub fn check_token(&self, expected: &str) -> Result<(), ProtocolError> {
        if expected.is_empty() {
            return Err(ProtocolError::Unauthorized {
                detail: "the core is configured with an EMPTY shared token, so no peer \
                         can be admitted; set a token in the daemon config"
                    .into(),
            });
        }
        match self.token.as_deref() {
            Some(t) if t == expected => Ok(()),
            Some(_) => Err(ProtocolError::Unauthorized {
                detail: "the shared token does not match the one this core was \
                         configured with"
                    .into(),
            }),
            None => Err(ProtocolError::Unauthorized {
                detail: "this core requires a shared token and the handshake carried \
                         none"
                    .into(),
            }),
        }
    }

    /// The human-facing line for a stamp comparison, ready to log verbatim. Names
    /// BOTH stamps whenever they can be compared, so the reader never has to go find
    /// the other half.
    pub fn stamp_report(&self, ours: &CoreStamp) -> String {
        match self.compare_stamp(ours) {
            StampComparison::Match => format!("core build stamp match ({ours})"),
            StampComparison::Differs => format!(
                "core build stamp DIFFERS: local {ours} vs remote {} - not a refusal \
                 (the protocol version is the gate), but the two peers are not the same \
                 build",
                self.stamp
            ),
            StampComparison::Unknown => format!(
                "core build stamp unknown (local {ours}, remote {}) - no claim either way",
                self.stamp
            ),
        }
    }
}

/// The core's answer to a [`Hello`] (v3): who you are talking to, and what you are
/// allowed to do.
///
/// One message, sent before any command traffic, so a client renders its controls
/// from a FACT rather than from an assumption. A UI that has not received this is not
/// yet connected; a UI that received [`LeaseAnswer::Observer`] must render read-only
/// controls, and if [`LeaseAnswer::was_downgraded`] it must say so rather than let the
/// operator infer it from a refusal later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloAck {
    /// The core's protocol version. First field (see [`VersionHeader`]) so a client
    /// can probe it even when the rest is unparseable.
    pub v: u16,
    /// Always [`PeerRole::Core`] in practice; carried so the ack is self-describing.
    pub role: PeerRole,
    /// The core's free-form build tag, for humans.
    pub build: String,
    /// The core's build stamp. Compared and REPORTED, never enforced - the client
    /// shows it next to its own so a bug report names both halves.
    #[serde(default = "CoreStamp::unknown")]
    pub stamp: CoreStamp,
    /// The lease decision (see [`crate::session`]).
    pub lease: LeaseAnswer,
    /// Seconds between heartbeats the core expects from this peer. The client pings
    /// at least this often; the core's controller-timeout is a multiple of it and is
    /// reported inside [`LeaseAnswer::Controller`].
    pub heartbeat_s: f64,
}

impl HelloAck {
    /// The role this ack grants.
    pub fn role(&self) -> RunRole {
        self.lease.role()
    }

    /// Enforce the exact-match version rule on a received ack. A client checks this
    /// before believing anything else in the message.
    pub fn check(&self) -> Result<(), ProtocolError> {
        check_version(self.v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{LeaseId, ObserverReason};

    #[test]
    fn same_version_is_accepted() {
        assert!(check_version(PROTOCOL_VERSION).is_ok());
        assert!(Hello::new(PeerRole::Ui, "test").check().is_ok());
    }

    #[test]
    fn mismatch_names_both_versions() {
        let err = check_version(PROTOCOL_VERSION + 7).unwrap_err();
        assert_eq!(
            err,
            ProtocolError::VersionMismatch {
                ours: PROTOCOL_VERSION,
                theirs: PROTOCOL_VERSION + 7,
            }
        );
        // The human-facing message carries both numbers - that is the "loudly" part.
        let msg = err.to_string();
        assert!(msg.contains(&format!("v{PROTOCOL_VERSION}")), "{msg}");
        assert!(msg.contains(&format!("v{}", PROTOCOL_VERSION + 7)), "{msg}");
    }

    /// Wire bump #5: the version is 6, and EVERY older peer is refused.
    ///
    /// The refusal is the whole point of a bump under an exact-match gate. A v5 peer
    /// handed a v6 payload would meet `TestProfile::Twr` as an unknown enum variant and
    /// the `Twr` command group the same way - mid-workflow, on the document that points
    /// a shaker - instead of at connect, where a version mismatch belongs. It would
    /// also meet `TestMode::Twr` inside a capability descriptor it asked for, which is
    /// the subtler half: a v5 UI decoding a v6 `ModeMatrix` fails on a struct field it
    /// has never heard of, so even a read-only peer is refused, and correctly.
    ///
    /// The loop below is the load-bearing half: it asserts EVERY older version is
    /// refused, not just the immediately preceding one, so a partial gate that
    /// happened to catch `PROTOCOL_VERSION - 1` cannot pass.
    #[test]
    fn the_wire_bump_landed_and_older_peers_are_refused() {
        assert_eq!(
            PROTOCOL_VERSION, 6,
            "TWR-on-NI, wire bump #5 - the TWR profile, the TWR command group with \
             its chunked reference upload, Response::Twr, SchemaKind::TwrReference, and \
             the first new TestMode variant this crate has ever taken"
        );
        for old in 1..PROTOCOL_VERSION {
            assert!(
                check_version(old).is_err(),
                "a v{old} peer must be refused at the gate, not part-way through a run"
            );
        }
    }

    /// THE TOKEN GATE. Three ways to fail it, each with its own message, and the one
    /// way to pass.
    ///
    /// RED-MUTATION: make the `expected.is_empty()` arm return `Ok(())` (i.e. read a
    /// blank configured token as "no authentication required") and the first
    /// assertion reds - which is the state where a mis-edited config file puts a
    /// shaker on an open port.
    #[test]
    fn the_shared_token_is_checked_and_a_blank_expectation_admits_nobody() {
        let with = Hello::new(PeerRole::Ui, "hertzforge-ui").with_token("s3cret");
        let without = Hello::new(PeerRole::Ui, "hertzforge-ui");

        assert!(with.check_token("s3cret").is_ok());

        // A blank configured token refuses EVERYONE, including a peer that also
        // offers a blank one.
        assert!(matches!(
            with.check_token("").unwrap_err(),
            ProtocolError::Unauthorized { .. }
        ));
        assert!(Hello::new(PeerRole::Ui, "x")
            .with_token("")
            .check_token("")
            .is_err());

        // Wrong token, and no token, are both refusals - with different detail so a
        // human can tell "you typed it wrong" from "your client is too old to send
        // one".
        let wrong = with.check_token("other").unwrap_err();
        let absent = without.check_token("s3cret").unwrap_err();
        assert!(wrong.to_string().contains("does not match"), "{wrong}");
        assert!(absent.to_string().contains("carried none"), "{absent}");
    }

    /// The handshake's defaults are the least-authority ones, and they survive a
    /// round-trip through a peer that omits every v3 field.
    #[test]
    fn a_v3_handshake_defaults_to_observer_with_no_token_and_no_downgrade() {
        let bare = Hello::new(PeerRole::Ui, "hertzforge-ui");
        assert_eq!(bare.run_role, RunRole::Observer);
        assert!(!bare.accept_observer_downgrade);
        assert_eq!(bare.token, None);

        let json = format!(r#"{{"v":{PROTOCOL_VERSION},"role":"ui","build":"x"}}"#);
        let decoded: Hello = serde_json::from_str(&json).expect("v3 fields are additive");
        assert_eq!(decoded.run_role, RunRole::Observer);
        assert!(!decoded.accept_observer_downgrade);
        assert_eq!(decoded.token, None);
        assert!(decoded.check().is_ok());
    }

    /// The builders compose and the ack round-trips with its lease decision intact.
    #[test]
    fn the_handshake_pair_round_trips_with_the_lease_decision() {
        let hello = Hello::new(PeerRole::Ui, "hertzforge-ui 0.1")
            .asking(RunRole::Controller)
            .accepting_observer_downgrade()
            .with_token("s3cret");
        assert_eq!(hello.run_role, RunRole::Controller);
        assert!(hello.accept_observer_downgrade);
        let back: Hello = serde_json::from_str(&serde_json::to_string(&hello).unwrap()).unwrap();
        assert_eq!(back, hello);

        for lease in [
            LeaseAnswer::Controller {
                lease: LeaseId(42),
                timeout_s: 30.0,
            },
            LeaseAnswer::Observer {
                reason: ObserverReason::Downgraded,
                controller: Some("10.0.0.4:51022".into()),
            },
        ] {
            let ack = HelloAck {
                v: PROTOCOL_VERSION,
                role: PeerRole::Core,
                build: "hertzforge-backendd 0.1".into(),
                stamp: CoreStamp::unknown(),
                lease,
                heartbeat_s: 5.0,
            };
            let back: HelloAck =
                serde_json::from_str(&serde_json::to_string(&ack).unwrap()).unwrap();
            assert_eq!(back, ack);
            assert!(back.check().is_ok());
            assert_eq!(back.role(), ack.lease.role());
        }
    }

    fn stamp(sha: &str, dirty: bool, host: &str, target: &str) -> CoreStamp {
        CoreStamp {
            sha: sha.into(),
            dirty,
            host: host.into(),
            target: target.into(),
        }
    }

    /// The load-bearing half of the stamp rule: a DIFFERING stamp does NOT refuse the
    /// connection. The version gate is the gate; the stamp is a diagnostic.
    ///
    /// Red-mutation: make `Hello::check` consult `compare_stamp` and refuse on
    /// `Differs`, and this reds - as it should, because that change would break every
    /// two-machine deployment.
    #[test]
    fn a_differing_build_stamp_does_not_refuse_the_connection() {
        let ours = stamp("a1b2c3d", false, "mac-host", "aarch64-apple-darwin");
        let remote = Hello::with_stamp(
            PeerRole::Core,
            "hertzforge-core 0.1",
            stamp("9999999", true, "linux-host", "x86_64-unknown-linux-gnu"),
        );
        assert!(
            remote.check().is_ok(),
            "same protocol version: the connection stands"
        );
        assert_eq!(remote.compare_stamp(&ours), StampComparison::Differs);
        // ... and the report names BOTH stamps so the human can act.
        let report = remote.stamp_report(&ours);
        assert!(report.contains("a1b2c3d"), "{report}");
        assert!(report.contains("9999999-dirty"), "{report}");
        assert!(report.contains("not a refusal"), "{report}");
    }

    /// The split topology - Mac front end, Linux core, same commit - is a MATCH.
    /// Host and target differ by design and must not read as a mismatch.
    #[test]
    fn the_mac_ui_linux_core_split_reports_a_match() {
        let mac = stamp("a1b2c3d", false, "mac-host", "aarch64-apple-darwin");
        let linux = Hello::with_stamp(
            PeerRole::Core,
            "hertzforge-core 0.1",
            stamp("a1b2c3d", false, "linux-host", "x86_64-unknown-linux-gnu"),
        );
        assert_eq!(linux.compare_stamp(&mac), StampComparison::Match);
        assert!(linux.stamp_report(&mac).contains("match"));
    }

    /// Same commit, one side DIRTY: not the same binary, and reported as differing.
    #[test]
    fn a_dirty_side_is_not_the_same_build() {
        let clean = stamp("a1b2c3d", false, "h", "t");
        let dirty = Hello::with_stamp(PeerRole::Core, "b", stamp("a1b2c3d", true, "h", "t"));
        assert_eq!(dirty.compare_stamp(&clean), StampComparison::Differs);
    }

    /// An absent stamp reports UNKNOWN, never a mismatch - and a `Hello` encoded
    /// without the field still decodes (the field is additive within v2).
    #[test]
    fn an_absent_stamp_reports_unknown_and_still_decodes() {
        let ours = stamp("a1b2c3d", false, "h", "t");
        let bare = Hello::new(PeerRole::Ui, "hertzforge-ui 0.1");
        assert_eq!(bare.compare_stamp(&ours), StampComparison::Unknown);
        assert!(bare.stamp_report(&ours).contains("unknown"));

        let json = format!(r#"{{"v":{PROTOCOL_VERSION},"role":"ui","build":"x"}}"#);
        let decoded: Hello = serde_json::from_str(&json).expect("stamp field is optional");
        assert!(decoded.stamp.is_unknown());
        assert!(decoded.check().is_ok());
    }

    /// The stamp round-trips on the wire in camelCase, matching
    /// `hertzforge_buildinfo::BuildInfo` (the type it mirrors) field for field.
    #[test]
    fn the_stamp_round_trips_on_the_wire() {
        let hello = Hello::with_stamp(
            PeerRole::Core,
            "hertzforge-core 0.1",
            stamp("a1b2c3d", true, "linux-host", "x86_64-unknown-linux-gnu"),
        );
        let text = serde_json::to_string(&hello).unwrap();
        let back: Hello = serde_json::from_str(&text).unwrap();
        assert_eq!(back, hello, "{text}");
        for field in ["\"sha\"", "\"dirty\"", "\"host\"", "\"target\""] {
            assert!(text.contains(field), "{text}");
        }
    }
}
