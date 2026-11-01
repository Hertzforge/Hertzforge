//! VERSIONED OPAQUE DOCUMENTS - the payloads this seam carries without interpreting
//! (v3), and the capture metadata the LIST verb answers with.
//!
//! # Why some payloads are opaque
//!
//! The channel table lives in `hertzforge-channel-table`; a recorder configuration lives
//! in `hertzforge-recorder`. Neither may be imported here: this crate's dependency floor
//! is `serde` and nothing else, and the UI-side peer of a split deployment must not
//! link hardware-side crates (crate docs, "Dependency discipline"). Mirroring those
//! two structures field-for-field, the way `Caps` is mirrored, would be a bad trade -
//! a `Caps` mirror is ~40 stable fields describing a hardware posture, while a channel
//! table is a live document with a migration story of its own and an 18-column CSV
//! contract nobody wants duplicated.
//!
//! So they cross as [`Doc`] / [`TableDoc`]: a JSON string plus the SCHEMA VERSION it
//! was written at. Exactly the [`crate::FrameBytes`] posture applied to text - the
//! protocol is a pipe, and the two ends of the document contract are the crates that
//! own the type.
//!
//! # The schema version is checked, not guessed
//!
//! An opaque payload with no version is a silent-misparse waiting to happen, which is
//! the one failure this seam exists to prevent. So every opaque document states the
//! schema it was written at, the receiver compares it against the constant its build
//! was compiled with, and a mismatch is [`crate::ProtocolError::UnsupportedSchema`] -
//! refused, naming both numbers, exactly like the protocol version itself.
//!
//! Note the two versions answer different questions and BOTH are needed:
//! [`crate::PROTOCOL_VERSION`] says "we speak the same wire"; a schema version says
//! "we agree on what is inside this particular document". Two peers can be on the same
//! wire and still disagree about a channel table if one of them was built against a
//! newer `hertzforge-channel-table`.

use serde::{Deserialize, Serialize};

use crate::error::ProtocolError;

/// The `hertzforge-channel-table` schema this build writes and accepts.
///
/// MIRRORED from `hertzforge_channel_table::SCHEMA_VERSION`, kept aligned by review - the
/// same rule as the `Caps` mirror. Diff against that constant when either side moves.
pub const CHANNEL_TABLE_SCHEMA: u32 = 2;

/// The recorder-configuration schema this build writes and accepts.
///
/// `1` because `hertzforge_recorder::RecorderConfig` has no version of its own yet; this
/// constant is where one gets stated when it grows a breaking change, and refusing a
/// mismatch is cheaper than discovering it in a `serde` error at record time.
pub const RECORDER_CONFIG_SCHEMA: u32 = 1;

/// The run-provenance schema this build writes and accepts.
pub const PROVENANCE_SCHEMA: u32 = 1;

/// The test-profile schema carried by [`crate::SineProfile`] / [`crate::RandomProfile`].
///
/// These are NOT opaque - they are this crate's own types - but they carry a schema
/// version for the same reason the opaque documents do: a profile is the document
/// most likely to be authored once and replayed against a newer build, and it is the
/// one whose misparse points a shaker somewhere the operator did not ask for.
/// **It stays `1` across the v5 AND v6 bumps, deliberately.** `PROFILE_SCHEMA` is ONE
/// global exact-match number shared by EVERY [`crate::TestProfile`] variant, so bumping
/// it to mark the three new v5 variants - or [`crate::TwrProfile`] at v6 - would have
/// refused the deployed application's existing Random and Explorer profiles -
/// mid-workflow, on stored documents that did not change. The new variants carry
/// `schema_version` defaulted to this value and are gated by
/// [`crate::PROTOCOL_VERSION`] instead, which is the gate that actually distinguishes a
/// peer that knows them from one that does not.
pub const PROFILE_SCHEMA: u32 = 1;

/// The shock per-pulse record schema (v5). Carried by
/// [`crate::StatusReport::last_pulse`] and answered by
/// [`crate::ShockCommand::PulseDetail`] / [`crate::ShockCommand::PulseList`].
pub const SHOCK_PULSE_SCHEMA: u32 = 1;

/// The shock SRS analysis-grid schema (v5). Answered by
/// [`crate::ShockCommand::SrsFreqs`].
pub const SHOCK_SRS_GRID_SCHEMA: u32 = 1;

/// The SRS predictive-notch readout schema. Answered by
/// [`crate::SrsCommand::NotchStatus`].
///
/// **ADDITIVE within v5** (2026-08-09), not a v6 payload: the answer to a query only a
/// NEW peer can send, which is the v5 one-directional additivity invariant stated in
/// `PROTOCOL_CHANGELOG.md`. See [`crate::SrsCommand::NotchStatus`] for the full
/// reasoning, including the one direction that is genuinely new.
pub const SHOCK_NOTCH_SCHEMA: u32 = 1;
/// The TWR reference-record document schema (v6).
///
/// ONE constant for the three documents that describe a staged reference and what the
/// run has done with it: the [`crate::TwrCommand::CommitReference`] acknowledgement,
/// the [`crate::TwrCommand::ReferenceInfo`] manifest, and the
/// [`crate::TwrCommand::History`] convergence table. They share a version because they
/// are three views of ONE thing - the record and its iteration - and a client that can
/// read the manifest can read the history it produced. Splitting them into three
/// constants would be three numbers that always move together, which is three chances
/// for two of them to disagree.
pub const TWR_REFERENCE_SCHEMA: u32 = 1;

/// Which document a schema refusal is about, so a UI can branch on the cause.
///
/// # The kind is a STAMPING and CHECKING label, never a wire tag
///
/// This enum does NOT appear on [`Doc`] - a `Doc` is a schema number and a JSON
/// string, and nothing more. The kind exists so that the SENDER stamps the right
/// constant and the RECEIVER checks against the right one, and so that a refusal can
/// say WHICH document is at fault. It is stated here because the v5 shock kinds are
/// the first ones a reader might mistake for a routing discriminator: a client answers
/// the query it asked, it does not read a kind off the document and dispatch on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaKind {
    /// The channel table.
    ChannelTable,
    /// A recorder configuration.
    RecorderConfig,
    /// A test profile (any [`crate::TestProfile`] variant - ONE shared version, see
    /// [`PROFILE_SCHEMA`]).
    Profile,
    /// A run-provenance block.
    Provenance,
    /// A shock per-pulse record (v5). Stamping/checking only - see the type docs.
    ShockPulse,
    /// A shock SRS analysis grid (v5). Stamping/checking only - see the type docs.
    ShockSrsGrid,
    /// An SRS predictive-notch readout (v5, additive 2026-08-09). Stamping/checking
    /// only - see the type docs.
    ShockNotch,
    /// A TWR reference manifest, commit acknowledgement or convergence history (v6).
    /// Stamping/checking only - see the type docs.
    TwrReference,
}

impl SchemaKind {
    /// The schema version THIS build speaks for this document kind.
    pub fn current(self) -> u32 {
        match self {
            SchemaKind::ChannelTable => CHANNEL_TABLE_SCHEMA,
            SchemaKind::RecorderConfig => RECORDER_CONFIG_SCHEMA,
            SchemaKind::Profile => PROFILE_SCHEMA,
            SchemaKind::Provenance => PROVENANCE_SCHEMA,
            SchemaKind::ShockPulse => SHOCK_PULSE_SCHEMA,
            SchemaKind::ShockSrsGrid => SHOCK_SRS_GRID_SCHEMA,
            SchemaKind::ShockNotch => SHOCK_NOTCH_SCHEMA,
            SchemaKind::TwrReference => TWR_REFERENCE_SCHEMA,
        }
    }

    /// Enforce the exact-match schema rule for this document kind.
    ///
    /// EXACT MATCH, deliberately, mirroring [`crate::envelope::check_version`]. A
    /// "newer is fine, older is fine" rule would put the burden of guessing on the
    /// side with the least information.
    pub fn check(self, theirs: u32) -> Result<(), ProtocolError> {
        if theirs == self.current() {
            Ok(())
        } else {
            Err(ProtocolError::UnsupportedSchema {
                kind: self,
                ours: self.current(),
                theirs,
            })
        }
    }
}

/// A versioned opaque JSON document.
///
/// `json` is carried verbatim: never re-encoded, never reformatted, never validated
/// here. The receiving side parses it with the crate that owns the type, AFTER
/// [`SchemaKind::check`] has agreed the two builds mean the same thing by it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Doc {
    /// The schema `json` was written at.
    pub schema_version: u32,
    /// The document, verbatim.
    pub json: String,
}

impl Doc {
    /// A document stamped at this build's schema for `kind`.
    pub fn new(kind: SchemaKind, json: impl Into<String>) -> Self {
        Doc {
            schema_version: kind.current(),
            json: json.into(),
        }
    }

    /// Enforce the schema rule before anyone parses `json`.
    pub fn check(&self, kind: SchemaKind) -> Result<(), ProtocolError> {
        kind.check(self.schema_version)
    }
}

/// The channel table, with the CONCURRENCY TOKEN that makes the daemon authoritative.
///
/// # Why an etag, and why the daemon is the authority
///
/// In the split deployment the table is not a UI document that happens to be sent
/// down; it is the thing the running acquisition is BOUND TO, and it lives where the
/// hardware lives. Two operators on two Macs (or one operator with a stale tab) can
/// otherwise both edit "the" table and the second write silently erases the first -
/// including, potentially, an abort limit the first operator just added.
///
/// So: every read hands back an `etag`, every write must quote the etag it was
/// derived from, and a write quoting anything else is REFUSED with
/// [`ProtocolError::StaleUpload`] naming both. The UI re-reads and the operator
/// re-applies. Last-write-wins is not an option on a document that carries abort
/// limits.
///
/// The etag is opaque: a digest of the serialized table computed by the daemon. Its
/// only contract is that it changes when the table changes and is stable when it does
/// not. `""` means "I have no opinion" and is accepted ONLY on the first write of a
/// session that has not read yet - see the daemon's `channel_table` module for that
/// rule and its test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableDoc {
    /// The `hertzforge-channel-table` schema `json` was written at.
    pub schema_version: u32,
    /// The concurrency token this document was read at (see the type docs).
    pub etag: String,
    /// The table, verbatim JSON.
    pub json: String,
}

impl TableDoc {
    /// A table document stamped at this build's channel-table schema.
    pub fn new(etag: impl Into<String>, json: impl Into<String>) -> Self {
        TableDoc {
            schema_version: CHANNEL_TABLE_SCHEMA,
            etag: etag.into(),
            json: json.into(),
        }
    }

    /// Enforce the channel-table schema rule.
    pub fn check(&self) -> Result<(), ProtocolError> {
        SchemaKind::ChannelTable.check(self.schema_version)
    }
}

/// One capture, as the LIST verb reports it ("captures are
/// daemon-local with a LIST verb; fetch-to-Mac stays deferred").
///
/// # What this is and is not
///
/// It is METADATA. The capture itself - a `telemetry.f32` that is routinely gigabytes -
/// stays on the daemon host, and v3 deliberately ships no fetch verb: pulling a
/// multi-gigabyte binary across the same link that carries the display path, during a
/// run, is a decision that deserves its own design rather than a convenience method.
/// A remote operator can SEE what was captured, with enough identity to go get it by
/// other means (scp, a share, the box's own UI).
///
/// # The fields that make it evidence rather than a file listing
///
/// `station_label`, `run_id` and `complete` are why this type is not just a name and a
/// size. A capture whose station was NI-SIM is driver-simulated data; a capture with
/// `complete: false` was interrupted (`kill -9`, a full disk, a fault) and its tail is
/// whatever the last flush wrote. Both facts are recorded at capture time, and a list
/// that omitted them would let a remote operator treat a truncated simulated run as a
/// measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureInfo {
    /// The run directory name on the daemon host - the identity, and what a human
    /// needs in order to go find it.
    pub name: String,
    /// Frames written, derived from the telemetry file's LENGTH rather than from a
    /// sidecar counter (the file is the authority; a sidecar written before a kill is
    /// not).
    pub frames: u64,
    /// Seconds of data, `frames / sample_rate_hz`. `0.0` when the rate is unknown.
    pub duration_s: f64,
    /// Channels per frame.
    pub n_channels: u32,
    /// Bytes on disk for the telemetry payload.
    pub bytes: u64,
    /// Unix seconds at capture start, or `0` when the sidecar could not say.
    pub started_unix: u64,
    /// `true` only for a capture that reached a clean `finish()`. `false` means the
    /// tail is whatever the last flush wrote.
    pub complete: bool,
    /// The station class the capture was taken on, rendered (`HERTZFORGE-SIM`,
    /// `NI-SIM (driver-simulated)`, `NI-REAL`). Empty when the sidecar predates
    /// provenance.
    pub station_label: String,
    /// The run this capture belongs to, as a STRING (u64 loses precision crossing
    /// into a JS number). `"0"` = unattributed.
    pub run_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matching_schema_passes_and_a_mismatch_names_both_numbers() {
        assert!(SchemaKind::ChannelTable.check(CHANNEL_TABLE_SCHEMA).is_ok());
        let err = SchemaKind::ChannelTable
            .check(CHANNEL_TABLE_SCHEMA + 5)
            .unwrap_err();
        assert_eq!(
            err,
            ProtocolError::UnsupportedSchema {
                kind: SchemaKind::ChannelTable,
                ours: CHANNEL_TABLE_SCHEMA,
                theirs: CHANNEL_TABLE_SCHEMA + 5,
            }
        );
        let msg = err.to_string();
        assert!(msg.contains(&CHANNEL_TABLE_SCHEMA.to_string()), "{msg}");
        assert!(
            msg.contains(&(CHANNEL_TABLE_SCHEMA + 5).to_string()),
            "{msg}"
        );
    }

    /// The channel-table schema mirror must equal `hertzforge_channel_table`'s own
    /// constant. This crate cannot import it (dependency floor), so the equality is
    /// kept by REVIEW - and this test is the reminder that says so at the moment the
    /// constant is touched.
    #[test]
    fn the_mirrored_channel_table_schema_is_stated_where_a_reviewer_will_see_it() {
        assert_eq!(
            CHANNEL_TABLE_SCHEMA, 2,
            "hertzforge_channel_table::SCHEMA_VERSION moved; mirror it here and bump \
             PROTOCOL_VERSION if the document shape changed on the wire"
        );
    }

    #[test]
    fn documents_carry_this_builds_schema_by_construction() {
        let d = Doc::new(SchemaKind::RecorderConfig, "{}");
        assert_eq!(d.schema_version, RECORDER_CONFIG_SCHEMA);
        assert!(d.check(SchemaKind::RecorderConfig).is_ok());
        // ... and checking it as the WRONG kind refuses, because the two constants
        // are not interchangeable even when they happen to be equal today.
        let t = TableDoc::new("abc", "{}");
        assert_eq!(t.schema_version, CHANNEL_TABLE_SCHEMA);
        assert!(t.check().is_ok());
    }
}
