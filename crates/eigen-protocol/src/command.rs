//! Operator commands - INTENT in engineering units, never device mechanics.
//!
//! Every verb here is something an operator means ("go to 500 Hz", "bring the level
//! to -6 dB", "run this profile"), expressed in engineering units. Nothing here names
//! a nudge, a step direction, a wire word, a block, a template, or a checksum - the
//! core-side delegate service owns the translation to whatever the active hardware
//! needs (paced nudge convergence on a delegated controller, absolute setpoints on NI). See the
//! crate docs for the full rule and the YES/NO examples.
//!
//! This is a REPRESENTATIVE near-term surface (run lifecycle, profile, level,
//! frequency, backend selection, status, streams) translated from the existing Tauri
//! command layer - a seam reservation, not a migration of all 225 commands. Growth is
//! additive under the version rule in [`crate::envelope`].

use serde::{Deserialize, Serialize};

use crate::caps::{ActionKind, BackendId, TestMode};
use crate::docs::{Doc, SchemaKind, TableDoc, PROFILE_SCHEMA};
use crate::error::ProtocolError;
use crate::frames::StreamKind;
use crate::shock::{ClassicalShockProfile, ShockCommand, SrsCommand, SrsProfile};
use crate::sine::{SineCommand, SineWireProfile};
use crate::subscribe::StreamSub;
use crate::twr::{TwrCommand, TwrProfile};

/// A client -> core command. See the module docs; the wire shape is the
/// default externally-tagged serde form so it survives non-self-describing codecs.
// Size comes from `SetProfile`'s unboxed `TestProfile::Random`; see the allow on `TestProfile`.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    /// Describe what the active backend can do ([`crate::Response::Capabilities`]).
    DescribeCapabilities,
    /// Report current run status ([`crate::Response::Status`]). Pull-based; the same
    /// report also arrives pushed via [`crate::Event::StateChanged`].
    GetStatus,
    /// List the backend registry rows ([`crate::Response::Backends`]).
    ListBackends,
    /// Select the active station/backend. Planned-but-not-installed rows are refused
    /// honestly ([`crate::ProtocolError::Refused`]).
    SelectBackend {
        /// The registry row to select.
        backend: BackendId,
    },
    /// Configure the test the operator wants to run. Intent-shaped: the profile is
    /// WHAT to run (a PSD, a sine plan, safety bands), never how to encode it.
    SetProfile {
        /// The test definition.
        profile: TestProfile,
    },
    /// Start the configured test.
    Start,
    /// End the test through the controlled ramp-down path (the normal operator stop).
    Stop,
    /// Immediate best-effort halt - the supervisory emergency verb. NOTE (crate docs):
    /// over this seam Abort is operator convenience on the supervisory path; the
    /// primary abort authority is on-hardware (device limits/interlock or the host
    /// watchdog co-resident with the drive clock), backed by the physical E-stop. The
    /// UI must never present a remote Abort as the safety mechanism.
    Abort,
    /// Bring the output level down to quiescent at the safe slew WITHOUT ending the
    /// run (converging on paced hardware).
    RampDown,
    /// Go to an absolute output level. Converges on paced hardware (the delegate
    /// paces toward it), applies immediately on setpoint hardware.
    SetLevel {
        /// The absolute target.
        level: LevelTarget,
    },
    /// Step the output level by a relative amount in dB - the coarse/fine level
    /// stepper intent. Engineering-units-relative is still intent ("+1 dB" is what
    /// the operator means); a raw device nudge count would be mechanics and is
    /// deliberately inexpressible here.
    AdjustLevel {
        /// Signed step, dB (dB is a pure ratio, so this is meaningful for both
        /// profile-relative and drive-volts level bases).
        delta_db: f64,
    },
    /// Hand the level schedule between the dwell timers and the operator, LIVE.
    /// This is the autopilot-disconnect/re-engage intent, not a profile
    /// edit: the transition is LEVEL-NEUTRAL in both directions (taking over must
    /// never move the shaker). `Auto -> Manual` freezes the schedule where it is
    /// and the level verbs drive from there; `Manual -> Auto` re-engages the
    /// ladder from the level the operator is currently at (nearest rung, dwell
    /// restarted), never from the top of the schedule.
    SetLadderMode {
        /// Who advances the schedule from now on.
        ladder: LadderMode,
    },
    /// Release a run HOLDING for operator review after system ID.
    ///
    /// The hold lives on the CORE, not in a front end: a UI-side pause would be
    /// lost on a disconnect and could let an unreviewed run proceed. Refused
    /// from any state other than the hold, because "proceed" arriving elsewhere
    /// means the two ends disagree about what the run is doing.
    ProceedAfterReview,
    /// Go to an absolute frequency. A setpoint write on host-synthesized hardware; a
    /// paced, re-measuring convergence (seconds) on device-paced hardware - watch
    /// [`crate::Event::IntentProgress`].
    GoToFrequency {
        /// Target frequency, Hz.
        hz: f64,
    },
    /// Step the frequency by a relative amount in Hz (the coarse/fine frequency
    /// stepper intent; same rationale as [`Command::AdjustLevel`]).
    AdjustFrequency {
        /// Signed step, Hz.
        delta_hz: f64,
    },
    /// Start delivery of display-frame streams to this client, with SELECTORS (v3).
    ///
    /// v2's `streams: Vec<StreamKind>` became `subs: Vec<StreamSub>`; the identical
    /// behavior is `StreamSub::all(kind)`. See [`crate::subscribe`] for why
    /// all-or-nothing stopped being viable at 400 channels, and for the rule that the
    /// selectors are honored by the PRODUCER (bytes not produced) rather than
    /// filtered at the consumer (bytes not shown).
    Subscribe {
        /// What to deliver, and how much of it.
        subs: Vec<StreamSub>,
    },
    /// Stop delivery of the named display-frame streams to this client.
    Unsubscribe {
        /// Streams to stop.
        streams: Vec<StreamKind>,
    },
    /// Read the CORE'S channel table (v3). The core is the authority in a split
    /// deployment - the table is what the running acquisition is bound to, and it
    /// lives where the hardware lives.
    GetChannelTable,
    /// Write the channel table, quoting the etag it was read at (v3). A write
    /// quoting a stale etag is refused with [`crate::ProtocolError::StaleUpload`]
    /// rather than silently overwriting another operator's edit - see [`TableDoc`].
    PutChannelTable {
        /// The table plus the etag this edit was derived from.
        table: TableDoc,
    },
    /// List the captures on the CORE host (v3).
    /// Metadata only - the capture bytes stay daemon-local and fetch-to-front-end is
    /// deliberately deferred. See [`crate::CaptureInfo`].
    ListCaptures,
    /// A Data Recorder verb (v3). The daemon-side module coverage for the arrival-day
    /// remote minimum; see [`RecorderCommand`].
    Recorder(RecorderCommand),
    /// An Explorer (open-loop sine survey) verb (v4). The sweep intents that have no
    /// expression in the general verb set; see [`ExplorerCommand`].
    Explorer(ExplorerCommand),
    /// A CLOSED-LOOP SINE verb (v5). Hold / resume / reverse / blend / feasibility
    /// override; see [`SineCommand`].
    Sine(SineCommand),
    /// A CLASSICAL SHOCK verb (v5). Fire, ladder, pulse queries, trains; see
    /// [`ShockCommand`].
    Shock(ShockCommand),
    /// An SRS verb (v5). Fire, ladder, pulse queries, system ID; see [`SrsCommand`].
    Srs(SrsCommand),
    /// A TIME WAVEFORM REPLICATION verb (v6). The iteration lifecycle, the level verbs
    /// and the chunked reference upload; see [`TwrCommand`].
    Twr(TwrCommand),
}

/// The Data Recorder lifecycle, as operator intent.
///
/// Same doctrine as [`Command`]: these are what the operator MEANS, in the recorder's
/// own vocabulary (monitor / arm / record / stop), not the service's internal state
/// machine. The configuration crosses as an opaque versioned [`Doc`] because
/// `eigen_recorder::RecorderConfig` may not be imported here (crate docs, dependency
/// discipline) and mirroring a live configuration struct field-for-field would be a
/// worse trade than mirroring `Caps`.
///
/// It was the only module group in v3; v4 adds [`ExplorerCommand`]. Every module
/// WITHOUT a group here is [`crate::ProtocolError::LocalOnly`] over this seam, stated
/// as a typed refusal so a remote UI renders "local-only in remote mode" rather than
/// silently falling back to a local backend - which would be a run on different
/// hardware than the operator believes.
///
/// A group exists here because a module needs verbs the general set cannot express
/// (the recorder's monitor/arm/record lifecycle; the Explorer's sweep). It is NOT a
/// claim that a given core SERVES the group: the wire surface and the daemon's
/// coverage are separate facts, and the daemon answers the second one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecorderCommand {
    /// Configure (and build) the recorder's station. MUTATING.
    Configure {
        /// `eigen_recorder::RecorderConfig` as JSON, at [`crate::RECORDER_CONFIG_SCHEMA`].
        config: Doc,
    },
    /// Begin monitoring (no recording). MUTATING.
    StartMonitor,
    /// Arm the trigger. MUTATING.
    Arm,
    /// Begin recording now. MUTATING.
    RecordNow,
    /// End the current recording, stay monitoring. MUTATING.
    StopRecording,
    /// Leave every live state. MUTATING.
    StopMonitor,
    /// Poll status. Read-only.
    Status,
    /// Poll the per-channel meters. Read-only.
    Meters,
    /// Poll the live FRF snapshot. Read-only.
    Frf,
    /// List the recordings this service knows about. Read-only.
    Recordings,
}

impl RecorderCommand {
    /// Does this verb change core state? The Observer gate consults
    /// [`Command::is_mutating`], which consults this.
    pub fn is_mutating(&self) -> bool {
        match self {
            RecorderCommand::Configure { .. }
            | RecorderCommand::StartMonitor
            | RecorderCommand::Arm
            | RecorderCommand::RecordNow
            | RecorderCommand::StopRecording
            | RecorderCommand::StopMonitor => true,
            RecorderCommand::Status
            | RecorderCommand::Meters
            | RecorderCommand::Frf
            | RecorderCommand::Recordings => false,
        }
    }
}

/// The Explorer's SWEEP intents (v4), the ones the general verb set cannot express.
///
/// Everything else the Explorer needs already exists as operator intent and stays
/// there: [`Command::GoToFrequency`] / [`Command::AdjustFrequency`] move the tone,
/// [`Command::SetLevel`] with [`LevelTarget::DriveVolts`] moves the drive,
/// [`Command::Start`] / [`Command::Stop`] run it. A parallel `ExplorerCommand::SetDrive`
/// would have been a second way to say the same thing, and two verbs for one intent is
/// how a UI and a core end up disagreeing about what the operator asked for.
///
/// A sweep is different: "traverse the band at N octaves per minute, in this direction,
/// until an edge or a stop" has no expression in a setpoint verb, and pinning it to a
/// stream of `GoToFrequency` calls would put the ramp on the network - which is the one
/// thing this seam does not do.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplorerCommand {
    /// Begin sweeping from the current frequency toward the band edge. MUTATING.
    ///
    /// Supersedes a sweep already running (the operator changed their mind about
    /// direction or rate); it is not an error to send it twice.
    StartSweep {
        /// Which band edge to travel toward.
        direction: SweepDirection,
        /// Traversal rate, octaves per minute. The core CLAMPS it to what the
        /// station can slew and reports what it actually used - a rate a shaker
        /// cannot follow must not be answered as if it were honored.
        rate_oct_per_min: f64,
    },
    /// Stop sweeping and HOLD the frequency where the sweep reached. MUTATING.
    ///
    /// Deliberately not "return to the start frequency": the operator stops a sweep
    /// at the moment they see something, and moving the tone off that spot destroys
    /// what they stopped to look at. It is also not a run stop - the drive stays where
    /// it is; [`Command::Stop`] ends the run.
    StopSweep,
}

impl ExplorerCommand {
    /// Does this verb change core state? The Observer gate consults
    /// [`Command::is_mutating`], which consults this. Both variants move (or stop
    /// moving) a shaker, so both are gated.
    pub fn is_mutating(&self) -> bool {
        match self {
            ExplorerCommand::StartSweep { .. } | ExplorerCommand::StopSweep => true,
        }
    }

    /// The [`ActionKind`] this verb is gated by in the capability
    /// [`crate::ActionMatrix`].
    ///
    /// Both map to [`ActionKind::GoToFrequency`] rather than adding a matrix row, the
    /// same trade as [`Command::SetLadderMode`] -> [`ActionKind::AdjustLevel`]: a
    /// backend that can be commanded to an absolute frequency is exactly one that can
    /// be walked between two of them, and every `ActionMatrix` literal in the tree
    /// would otherwise grow a field with identical support semantics.
    pub fn action_kind(&self) -> ActionKind {
        match self {
            ExplorerCommand::StartSweep { .. } | ExplorerCommand::StopSweep => {
                ActionKind::GoToFrequency
            }
        }
    }
}

/// Which band edge a sweep travels toward.
///
/// MIRRORED from the delegated Explorer service's `SweepDirection`, never imported - the same
/// dependency-floor rule that keeps `eigen-hal` out of this crate (see the crate docs).
/// Keep the two in step when either side changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepDirection {
    /// Toward the band HIGH edge.
    Up,
    /// Toward the band LOW edge.
    Down,
}

impl Command {
    /// The [`ActionKind`] this command corresponds to in the capability
    /// [`crate::ActionMatrix`], or `None` for transport-universal commands (status,
    /// discovery, subscription, backend selection) that are always available.
    pub fn action_kind(&self) -> Option<ActionKind> {
        match self {
            Command::SetProfile { .. } => Some(ActionKind::SetProfile),
            Command::Start => Some(ActionKind::Start),
            Command::Stop => Some(ActionKind::Stop),
            Command::Abort => Some(ActionKind::Abort),
            Command::RampDown => Some(ActionKind::RampDown),
            Command::SetLevel { .. } => Some(ActionKind::SetLevel),
            Command::AdjustLevel { .. } => Some(ActionKind::AdjustLevel),
            // The ladder mode is level-schedule AUTHORITY: a backend that serves
            // live level intents (AdjustLevel any time) is exactly one that can
            // hand the schedule to the operator mid-run. Mapped rather than
            // matrixed so the ActionMatrix struct (and every literal of it) does
            // not grow a field for a verb with identical support semantics.
            Command::SetLadderMode { .. } => Some(ActionKind::AdjustLevel),
            // Releasing a review hold is the run STARTING to drive, so it is
            // gated exactly like Start.
            Command::ProceedAfterReview => Some(ActionKind::Start),
            Command::GoToFrequency { .. } => Some(ActionKind::GoToFrequency),
            Command::AdjustFrequency { .. } => Some(ActionKind::AdjustFrequency),
            // A sweep is frequency AUTHORITY, mapped rather than matrixed - see
            // [`ExplorerCommand::action_kind`] for why it is not its own row.
            Command::Explorer(e) => Some(e.action_kind()),
            // v5. The module groups map onto existing rows for the same reason the
            // Explorer's do; the two shock groups answer `None` for their read-only
            // queries and their refused capture verbs, which have no matrix row
            // and must not claim one.
            Command::Sine(s) => Some(s.action_kind()),
            Command::Shock(s) => s.action_kind(),
            Command::Srs(s) => s.action_kind(),
            // v6. Mapped onto the EXISTING rows for the same reason every group above
            // is, including the upload verbs -> `SetProfile`: staging the record a
            // profile names is profile authority. The two read-only queries answer
            // `None` and must not claim a row.
            Command::Twr(t) => t.action_kind(),
            Command::DescribeCapabilities
            | Command::GetStatus
            | Command::ListBackends
            | Command::SelectBackend { .. }
            | Command::Subscribe { .. }
            | Command::Unsubscribe { .. }
            | Command::GetChannelTable
            | Command::PutChannelTable { .. }
            | Command::ListCaptures
            | Command::Recorder(_) => None,
        }
    }

    /// **Does this command change CORE state?** THE run-lease gate (v3).
    ///
    /// This is the single place the one-Controller rule is expressed, so that
    /// "Observers are read-only" is one exhaustive match a reviewer can read rather
    /// than a condition repeated at every handler. A new verb has to answer this
    /// question to compile - which is the point, because the failure mode of
    /// forgetting is an Observer that can drive a shaker.
    ///
    /// Judgement calls, stated:
    ///
    /// * [`Command::Abort`] IS mutating. It is supervisory-only over this seam (crate
    ///   docs) and never the primary abort authority, but that
    ///   is an argument about what it is FOR, not about whether it changes state.
    ///   Letting an Observer abort a run someone else is driving is exactly the
    ///   two-authorities confusion the lease exists to prevent, and the physical
    ///   E-stop - which is in the room with the hardware, not on a remote laptop - is
    ///   the answer for anyone without the lease.
    /// * [`Command::SelectBackend`] IS mutating. It rebinds what the next run is on.
    /// * [`Command::PutChannelTable`] IS mutating. It rewrites the document that
    ///   carries the abort limits.
    /// * [`Command::Subscribe`] / [`Command::Unsubscribe`] are NOT. They change what
    ///   THIS connection is sent and touch nothing another peer can observe - an
    ///   Observer that could not subscribe could not observe.
    pub fn is_mutating(&self) -> bool {
        match self {
            Command::SetProfile { .. }
            | Command::Start
            | Command::Stop
            | Command::Abort
            | Command::RampDown
            | Command::SetLevel { .. }
            | Command::AdjustLevel { .. }
            | Command::SetLadderMode { .. }
            | Command::ProceedAfterReview
            | Command::GoToFrequency { .. }
            | Command::AdjustFrequency { .. }
            | Command::SelectBackend { .. }
            | Command::PutChannelTable { .. } => true,
            Command::Recorder(r) => r.is_mutating(),
            Command::Explorer(e) => e.is_mutating(),
            Command::Sine(s) => s.is_mutating(),
            Command::Shock(s) => s.is_mutating(),
            Command::Srs(s) => s.is_mutating(),
            Command::Twr(t) => t.is_mutating(),
            Command::DescribeCapabilities
            | Command::GetStatus
            | Command::ListBackends
            | Command::Subscribe { .. }
            | Command::Unsubscribe { .. }
            | Command::GetChannelTable
            | Command::ListCaptures => false,
        }
    }

    /// Check every versioned document this command carries, before anything acts on
    /// it (v3). `Ok(())` for commands that carry none.
    ///
    /// EXHAUSTIVE, with no `_` arm (v4). It used to end in a wildcard, which is the
    /// shape where a new document-carrying verb compiles clean and silently skips its
    /// schema gate. Listing every variant costs a line each
    /// and makes forgetting a compile error instead of a runtime surprise.
    pub fn check_schemas(&self) -> Result<(), ProtocolError> {
        match self {
            Command::SetProfile { profile } => profile.check_schema(),
            Command::PutChannelTable { table } => table.check(),
            Command::Recorder(RecorderCommand::Configure { config }) => {
                config.check(SchemaKind::RecorderConfig)
            }
            // Carry no versioned document. Stated one by one so that a verb which
            // GAINS one cannot inherit a silent pass.
            Command::Recorder(
                RecorderCommand::StartMonitor
                | RecorderCommand::Arm
                | RecorderCommand::RecordNow
                | RecorderCommand::StopRecording
                | RecorderCommand::StopMonitor
                | RecorderCommand::Status
                | RecorderCommand::Meters
                | RecorderCommand::Frf
                | RecorderCommand::Recordings,
            )
            | Command::Explorer(ExplorerCommand::StartSweep { .. } | ExplorerCommand::StopSweep)
            // v5 module groups. None of them carries a versioned document TODAY -
            // their payloads are typed scalars and the profiles ride `SetProfile` -
            // and they are enumerated variant by variant for exactly the reason the
            // v4 comment above gives: a verb that GAINS a `Doc` must not inherit a
            // silent pass. `PulseDetail`/`PulseList`/`SrsFreqs` ANSWER with documents,
            // which is the core's stamping job, not this gate's.
            | Command::Sine(
                SineCommand::Hold
                | SineCommand::Resume
                | SineCommand::Reverse
                | SineCommand::SetBlendW { .. }
                | SineCommand::OverrideFeasibility { .. },
            )
            | Command::Shock(
                ShockCommand::FirePulse
                | ShockCommand::RunAuto { .. }
                | ShockCommand::JumpToLevel { .. }
                | ShockCommand::ArmWithOverride { .. }
                | ShockCommand::Reset
                | ShockCommand::PulseList
                | ShockCommand::PulseDetail { .. }
                | ShockCommand::SrsFreqs
                | ShockCommand::FireTrain { .. }
                | ShockCommand::FireGunfireBurst
                | ShockCommand::StartCapture
                | ShockCommand::StopCapture
                | ShockCommand::SaveSnapshot,
            )
            | Command::Srs(
                SrsCommand::FirePulse
                | SrsCommand::RunAuto { .. }
                | SrsCommand::JumpToLevel { .. }
                | SrsCommand::ArmWithOverride { .. }
                | SrsCommand::Reset
                | SrsCommand::RunSysId
                | SrsCommand::PulseList
                | SrsCommand::PulseDetail { .. }
                | SrsCommand::SrsFreqs
                | SrsCommand::NotchStatus
                | SrsCommand::StartCapture
                | SrsCommand::StopCapture
                | SrsCommand::SaveSnapshot,
            )
            // v6, the TWR group. Enumerated verb by verb under the same rule, and the
            // three UPLOAD verbs are the reason the rule earns its keep here: they are
            // the most document-shaped payloads on this wire and they are still NOT
            // versioned documents. `BeginReference` carries scalars, `ReferenceChunk`
            // carries a base64 STRING, and `CommitReference` carries nothing - the
            // record's identity is its declared sha, which the daemon checks against
            // what it staged rather than against a schema number. If any of them ever
            // gains a `Doc`, this arm has to move it out, and listing them is what
            // makes forgetting a compile error.
            | Command::Twr(
                TwrCommand::IterateOnce
                | TwrCommand::BeginPlayback
                | TwrCommand::StepLevel { .. }
                | TwrCommand::JumpLevel { .. }
                | TwrCommand::FreezeAdaptation { .. }
                | TwrCommand::Reset
                | TwrCommand::BeginReference { .. }
                | TwrCommand::ReferenceChunk { .. }
                | TwrCommand::CommitReference {}
                | TwrCommand::ReferenceInfo
                | TwrCommand::History,
            )
            | Command::DescribeCapabilities
            | Command::GetStatus
            | Command::ListBackends
            | Command::SelectBackend { .. }
            | Command::Start
            | Command::Stop
            | Command::Abort
            | Command::RampDown
            | Command::SetLevel { .. }
            | Command::AdjustLevel { .. }
            | Command::SetLadderMode { .. }
            | Command::ProceedAfterReview
            | Command::GoToFrequency { .. }
            | Command::AdjustFrequency { .. }
            | Command::Subscribe { .. }
            | Command::Unsubscribe { .. }
            | Command::GetChannelTable
            | Command::ListCaptures => Ok(()),
        }
    }
}

/// An absolute output-level target, in the two engineering bases operators actually
/// use. Which base a backend/mode accepts is a runtime matter
/// ([`crate::ProtocolError::InvalidArgument`] otherwise).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LevelTarget {
    /// dB relative to the profile's full level (the closed-loop convention: 0 = full
    /// level, -6 = half amplitude). Matches the Random schedule-ladder convention.
    Db(f64),
    /// Volts at the drive output (the open-loop convention: in open-loop sine the
    /// operator commands drive and measures response, so volts IS the intent - an
    /// absolute engineering quantity, not a device mechanic).
    DriveVolts(f64),
}

/// The test the operator wants to run.
///
/// ~~"Shock / SRS / MIMO profiles are deliberately absent until their surfaces
/// stabilize."~~ **CORRECTED at v5**: closed-loop sine, classical shock and SRS all
/// landed in one batched bump, because with an exact-match version gate there is no
/// graceful degradation and three separate bumps would have been three coordinated
/// redeploys. **v6 adds [`TestProfile::Twr`]** - one module this time, because it was
/// the only surface waiting when the bump was taken. MIMO is still absent, and
/// genuinely so - no module serves it.
///
/// A wire shape existing here is NOT a claim that a core SERVES it. The daemon answers
/// that question with a typed refusal per variant until the matching host lands; see
/// `crates/eigen-backendd/src/core.rs`.
// Wire type: `Random` is left unboxed; boxing it like `Sine` would change the public payload type.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestProfile {
    /// Open-loop sine: commanded drive, measured response.
    ///
    /// **DEAD WIRE SURFACE as of v4, kept deliberately.** No core serves it: the
    /// delegated open-loop path is local-only, and the host-synthesized survey is
    /// [`TestProfile::Explorer`]. It is retained rather than removed because the delegated
    /// variant is a circle-back once NI is functional (decided 2026-08-08), and
    /// removing it would spend a version bump now and another one to put it back.
    OpenLoopSine(SineProfile),
    /// Closed-loop random: spectral control to a PSD reference.
    Random(RandomProfile),
    /// Host-synthesized open-loop survey (v4): a commanded tone the operator moves in
    /// frequency and drive, optionally swept, with the response measured.
    Explorer(ExplorerProfile),
    /// CLOSED-LOOP SINE (v5): a control profile the servo holds a tracked tone to,
    /// under one of four frequency schedulers. See [`SineWireProfile`].
    ///
    /// The payload is deliberately NOT called `SineProfile` - that name is taken by
    /// the dead delegated open-loop payload above, and two types called the same thing on
    /// one wire is how a translation ends up reading the wrong one.
    ///
    /// **BOXED**, like the two below and like [`crate::Response::Capabilities`]: these
    /// three are by far the largest payloads this enum carries, and an unboxed variant
    /// makes every `Command` value - including a bare `Start` - as big as the biggest
    /// profile. `Box` is transparent to serde, so the wire shape is unchanged.
    Sine(Box<SineWireProfile>),
    /// CLASSICAL SHOCK (v5): an authored pulse, fired one at a time or in trains, and
    /// judged in the time domain against a standard. See [`ClassicalShockProfile`].
    /// Boxed - see [`TestProfile::Sine`].
    ShockClassical(Box<ClassicalShockProfile>),
    /// SRS (v5): a target response spectrum, synthesized and matched. See
    /// [`SrsProfile`]. Boxed - see [`TestProfile::Sine`].
    Srs(Box<SrsProfile>),
    /// TIME WAVEFORM REPLICATION (v6): a recorded transient replicated by iterating an
    /// inverse-filtered drive between plays. See [`TwrProfile`]. Boxed - see
    /// [`TestProfile::Sine`].
    ///
    /// **The reference SAMPLES are not in here.** They cross once, on their own,
    /// through the [`TwrCommand`] upload group, and this profile names the staged
    /// record by its digest. See [`crate::twr`]'s module docs for why a megabyte on
    /// `SetProfile` was the wrong shape.
    Twr(Box<TwrProfile>),
}

impl TestProfile {
    /// The [`TestMode`] this profile runs under, for checking against the
    /// [`crate::ModeMatrix`] before accepting.
    pub fn mode(&self) -> TestMode {
        match self {
            // The Explorer IS open-loop sine as far as the mode matrix is concerned -
            // commanded drive, measured response, nothing correcting per tick. What
            // differs is WHO synthesizes (the host, not the device), which is the
            // resolver's `DriveOrigin` axis and not a mode.
            TestProfile::OpenLoopSine(_) | TestProfile::Explorer(_) => TestMode::OpenLoopSine,
            TestProfile::Random(_) => TestMode::Random,
            // CLOSED-loop sine is its own mode, unlike the Explorer: something
            // corrects the drive every tick, which is precisely the distinction
            // `TestMode` draws and the one the shared-clock precondition binds to.
            TestProfile::Sine(_) => TestMode::Sine,
            TestProfile::ShockClassical(_) => TestMode::Shock,
            TestProfile::Srs(_) => TestMode::Srs,
            // Its OWN mode, and not one of the shock pair: TWR replicates a recorded
            // transient by CORRECTING between plays, where a classical pulse is fired
            // open-loop and an SRS pulse is synthesized offline. The mode matrix is
            // where a station says whether it can run that, and folding TWR into
            // `Shock` would let a station's shock claim answer for a mode it never
            // proved. Note the mirror exception in [`crate::caps`]: `TestMode::Twr` has
            // no `NativeMode` counterpart, so the claim can only come from a ledger.
            TestProfile::Twr(_) => TestMode::Twr,
        }
    }

    /// The schema version this profile was written at.
    pub fn schema_version(&self) -> u32 {
        match self {
            TestProfile::OpenLoopSine(p) => p.schema_version,
            TestProfile::Random(p) => p.schema_version,
            TestProfile::Explorer(p) => p.schema_version,
            TestProfile::Sine(p) => p.schema_version,
            TestProfile::ShockClassical(p) => p.schema_version,
            TestProfile::Srs(p) => p.schema_version,
            TestProfile::Twr(p) => p.schema_version,
        }
    }

    /// Enforce the exact-match profile-schema rule (v3).
    pub fn check_schema(&self) -> Result<(), ProtocolError> {
        SchemaKind::Profile.check(self.schema_version())
    }
}

/// The default for the profiles' `schema_version` field: what a payload written
/// before the field existed means.
///
/// It is [`PROFILE_SCHEMA`] rather than `0`, because within a single
/// [`crate::PROTOCOL_VERSION`] every peer is by definition built from the same wire
/// contract - the exact-match gate already refused everyone else. The field earns its
/// keep across a FUTURE bump, where a stored profile replayed by a newer build must
/// declare what it was written at.
pub(crate) fn profile_schema() -> u32 {
    PROFILE_SCHEMA
}

/// An open-loop sine plan: where to sit (or start), and the operator's safety
/// envelope. The envelope fields are intents ("never past these edges"), which the
/// core enforces however its hardware requires (predictive guard on paced hardware,
/// synthesis limits on setpoint hardware).
///
/// # `deny_unknown_fields` is the v3 unknown-field diagnostic
///
/// A field this build does not recognize is a hard, typed DECODE FAILURE naming the
/// field (serde's own "unknown field `x`, expected one of ..."), which the transport
/// carries back as [`ProtocolError::MalformedPayload`]. It is deliberately not an
/// accept-and-report: this is the payload that points a shaker, the crate's whole
/// doctrine is that a misparsed operator command is worse than a refused one, and
/// "we ignored part of your profile and ran anyway" is precisely the silent
/// best-effort parse [`crate::envelope`] bans at the version layer. Refusing also
/// costs nothing in codec support - unlike a `#[serde(flatten)]` capture map, which
/// would have broken this crate's stated ability to ride non-self-describing codecs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineProfile {
    /// The profile-schema version this was written at (v3). See [`PROFILE_SCHEMA`].
    #[serde(default = "profile_schema")]
    pub schema_version: u32,
    /// Initial frequency, Hz.
    pub start_hz: f64,
    /// Operator-authorized frequency band (low, high), Hz: level/frequency intents
    /// clamp at these edges rather than crossing them.
    pub band_hz: Option<(f64, f64)>,
    /// Operator-authorized drive ceiling, volts: no intent may converge past it.
    pub max_drive_v: Option<f64>,
}

/// A HOST-SYNTHESIZED open-loop survey plan (v4).
///
/// # Why this is a new variant and not four more fields on [`SineProfile`]
///
/// `SineProfile` is `deny_unknown_fields`, which makes growing it the opposite of
/// additive: a peer built before the new field decodes the payload as a hard field
/// error, mid-workflow, on the document that points a shaker. A new variant is refused
/// cleanly by the version gate at CONNECT instead - the peer never gets far enough to
/// misread anything. That trade is the whole reason v4 exists.
///
/// # The arm posture is written into the type
///
/// `amplitude_vpk` is NOT optional and NOT defaulted, and `0.0` is a legal, expected
/// value: the Explorer arms at zero drive and the operator raises it. That is what
/// makes a review hold unnecessary - nothing moves until a human commands a level -
/// so the field being present and explicit is load-bearing rather than a formality.
/// `max_drive_vpk` is the operator's ceiling and is enforced whatever the station is.
///
/// # Rates and bands are REQUESTS
///
/// `fs_hz` is what the station is asked for; a delta-sigma front end coerces it up its
/// own ladder and the run reports what it actually got
/// ([`crate::StatusReport::actual_fs_hz`]). Frequency intents clamp to
/// `band_hz` intersected with what the ADOPTED rate allows, and the clamped value is
/// reported back - a request that got clamped and a request that was honored must
/// never look the same to the operator.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplorerProfile {
    /// The profile-schema version this was written at. See [`PROFILE_SCHEMA`].
    #[serde(default = "profile_schema")]
    pub schema_version: u32,
    /// Requested sample rate, Hz. The station may COERCE it (see the type docs).
    pub fs_hz: f64,
    /// Operator-authorized frequency band `(low, high)`, Hz. Every frequency intent
    /// clamps here; the sweep travels between these edges and stops at them.
    pub band_hz: (f64, f64),
    /// Frequency to arm at, Hz. Clamped into `band_hz`.
    pub start_hz: f64,
    /// Drive amplitude to arm at, volts PEAK. **`0.0` = armed quiet**, the standard
    /// posture: the tone exists, the output is silent, and the operator raises it.
    pub amplitude_vpk: f64,
    /// Operator-authorized drive ceiling, volts PEAK. No intent may pass it.
    pub max_drive_vpk: f64,
    /// How fast frequency and drive are allowed to move. See [`ExplorerSlew`].
    pub slew: ExplorerSlew,
    /// Start sweeping as soon as the run engages. `None` = park at `start_hz` and
    /// wait for [`ExplorerCommand::StartSweep`], which is the usual case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep: Option<SweepSpec>,
    /// FFT size for the DISPLAY spectra, samples.
    ///
    /// Display-only, and named so. It sets what the operator sees, never what the
    /// synthesis does: an open-loop survey has no control estimate to smooth, so
    /// there is no `n_avg` here and no line grid the drive depends on.
    pub display_nfft: usize,
}

/// The rate limits every Explorer intent is paced by.
///
/// Stated in engineering units PER SECOND, not per tick. A tick-denominated slew is a
/// device mechanic wearing an intent's clothes: it silently changes meaning when the
/// block size or the adopted sample rate changes, which is exactly the class of drift
/// this seam exists to keep off the wire. The core converts these to per-frame
/// increments from the rate it ACTUALLY got.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplorerSlew {
    /// Frequency slew ceiling, octaves per second.
    pub freq_oct_per_s: f64,
    /// Drive slew ceiling, dB per second. Applies to raising AND lowering, which is
    /// what makes every stop a ramp rather than a cut.
    pub drive_db_per_s: f64,
}

/// A sweep plan: which way, and how fast.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepSpec {
    /// Which band edge to travel toward.
    pub direction: SweepDirection,
    /// Traversal rate, octaves per minute. Clamped by
    /// [`ExplorerSlew::freq_oct_per_s`]; a sweep may not outrun the slew limit.
    pub rate_oct_per_min: f64,
}

/// A closed-loop random plan: reference PSD, level schedule, duration.
///
/// `deny_unknown_fields` for the same reason as [`SineProfile`] - see its docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RandomProfile {
    /// The profile-schema version this was written at (v3). See [`PROFILE_SCHEMA`].
    #[serde(default = "profile_schema")]
    pub schema_version: u32,
    /// The reference PSD as breakpoints (anchors and slopes), with optional per-row
    /// alarm/abort bands.
    pub breakpoints: Vec<PsdBreakpoint>,
    /// The level ladder, run top-down; entries in dB relative to full level.
    pub schedule: Vec<ScheduleEntry>,
    /// Whether the ladder advances automatically or on operator level commands.
    pub ladder: LadderMode,
    /// Full-level test duration, seconds (`None` = run until stopped).
    pub duration_s: Option<f64>,
    /// Acquisition properties the operator chose (2026-08-02). ADDITIVE: absent
    /// from an older front end, in which case the core keeps its own defaults
    /// rather than inventing a rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acquisition: Option<AcquisitionProps>,
    /// Control-loop parameters the operator chose (2026-08-02). ADDITIVE, same
    /// rule as `acquisition`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<ControlProps>,
    /// Per-channel response/force LIMIT profiles - the notch tables
    /// (2026-08-03). Until this existed the Limits tab was applied on the local
    /// path only: a remote operator who authored a notch got NO notch, with no
    /// indication, which is the "looks armed and is not" class.
    ///
    /// Carried as the AUTHORED breakpoint rows, not as a per-line vector: the
    /// line grid is a function of `acquisition` (nPts, fs), so the only end
    /// that can interpolate onto the grid the run will actually use is the one
    /// that owns the grid. Sending a pre-interpolated vector would bake in the
    /// sender's assumed grid and silently mis-place every ceiling the moment
    /// the operator changed a block size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Vec<LimitProfileRows>>,
    /// Per-channel control weights, `(channel_index, weight)`. Absent = equal
    /// weights. (Previously local-path only.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_weights: Option<Vec<(u32, f64)>>,
    /// How multiple control channels combine: `weighted_avg` / `min` / `max`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composite_mode: Option<String>,
    /// FEASIBILITY inputs (2026-08-03): the selected shaker
    /// record, the moving-mass stack and the optional article geometry. Absent
    /// = no shaker selected, and the core SKIPS feasibility with an advisory
    /// note - which is exactly what every remote run did before this existed.
    ///
    /// TYPED, not opaque JSON: this crate's discipline is that serde is its
    /// only runtime dependency, so it carries the scalars feasibility needs
    /// rather than taking on the shaker database (or serde_json) to move a
    /// whole record. The core rebuilds its own types from these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feasibility: Option<FeasibilityInputs>,
    /// FRF pairs to measure and publish alongside the control loop.
    ///
    /// Absent or empty = only the sysID drive->control FRF, which is what the
    /// module published before this existed. Each pair costs one Welch
    /// cross-spectrum per control loop and one trace block on the wire; the
    /// core refuses more than [`MAX_FRF_PAIRS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frf_pairs: Option<Vec<FrfPairSpec>>,
}

/// The most FRF pairs one run may request.
///
/// The honest number, not "unlimited": each pair owns a 3-id trace block in
/// the 940..999 range (magnitude / coherence / phase), so twenty is where the
/// block runs out. The operator asked for unlimited and practically wants ~4
/// per plot across several plots; twenty covers that with room, and a stated
/// cap beats a promise that silently degrades.
pub const MAX_FRF_PAIRS: usize = 20;

/// The first trace id of the Random FRF-pair block. Pair `k` publishes
/// `FRF_PAIR_BASE + 3k` (magnitude), `+1` (coherence), `+2` (phase).
///
/// Deliberately NOT 930: 930/931/932 are the RECORDER's reserved FRF
/// magnitude / coherence / PSD-marker ids, and a `930 + 3k` scheme would have
/// put pair 0 exactly on top of them.
pub const FRF_PAIR_BASE: u32 = 940;

/// What a pair's REFERENCE signal is.
///
/// The drive is not an input channel and cannot be named by a channel index,
/// so a plain `(u32, u32)` pair would have made the most common FRF - drive to
/// response, the one sysID itself measures - unexpressible.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "ch")]
pub enum FrfSource {
    /// The synthesized drive. Needs the measured pipeline lag applied.
    Drive,
    /// An acquired input channel. Shares the acquisition clock with the
    /// response, so NO lag shift is applied.
    Channel(u32),
}

/// One requested FRF measurement.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FrfPairSpec {
    /// The reference (input) signal: the synthesized drive or an acquired channel.
    pub reference: FrfSource,
    /// The response channel index (must be enabled in the channel table).
    pub response_ch: u32,
}

/// What the shaker-limits check needs to run remotely.
///
/// Absent = no shaker selected, and the core skips feasibility with an
/// advisory note - which is what EVERY remote run did before this existed:
/// the operator picked a shaker in Shaker & Safety and the check silently did
/// not happen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeasibilityInputs {
    /// The selected shaker record, serialized by the sender.
    ///
    /// Carried as its own serialized form rather than re-typed field by field:
    /// each rating has a BASIS (rms / peak / sine-equivalent) that decides what
    /// the number means, and a flattened copy would have to guess it - which is
    /// the class of silent mistranslation this whole ticket exists to end. The
    /// core parses it into the real record; a payload that does not parse is a
    /// REFUSAL, never a run with feasibility quietly skipped. This crate keeps
    /// serde as its only runtime dependency, so it moves the string.
    pub shaker_json: String,
    /// The moving-mass stack in pounds, itemized the way the operator entered
    /// it so the report can attribute the total.
    pub armature_lb: f64,
    /// Slip-table mass in pounds.
    pub slip_table_lb: f64,
    /// Drive-adapter mass in pounds.
    pub drive_adapter_lb: f64,
    /// Fixturing mass in pounds.
    pub fixturing_lb: f64,
    /// Test-article mass in pounds.
    pub test_article_lb: f64,
}

/// One channel's authored limit profile: the freq/ceiling rows the operator
/// typed, plus what the limit protects. Interpolated onto the run's line grid
/// by the core (see [`RandomProfile::limits`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitProfileRows {
    /// The channel this profile limits.
    pub channel_index: u32,
    /// `response` (accel) or `force`. Both notch identically; the kind is the
    /// report/UI label.
    pub kind: String,
    /// Authored `(freq_hz, ceiling)` rows, ascending in frequency. The ceiling
    /// is a PSD in the channel's EU domain (`g^2/Hz` or `force^2/Hz`).
    pub rows: Vec<LimitRow>,
}

/// One authored limit breakpoint.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LimitRow {
    /// Breakpoint frequency in Hz.
    pub freq_hz: f64,
    /// The limit ceiling at `freq_hz`, a PSD in the channel's EU domain.
    pub ceiling: f64,
}

/// What the operator chose in the Data Acquisition block.
///
/// `df` and the line counts are DERIVED from these and are deliberately not on
/// the wire: a transported derived value is a second source of truth that can
/// disagree with the first.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcquisitionProps {
    /// Requested sample rate, Hz. The station may COERCE this (the NI 9234 has
    /// a 13.1072 MHz/(256n) ladder); what was actually programmed comes back
    /// through the configured-actual path, and the UI shows THAT.
    pub fs_hz: f64,
    /// FFT block size, samples.
    pub n_pts: usize,
    /// Spectral averages per control estimate.
    pub n_avg: usize,
}

/// The operator-facing control-loop parameters.
///
/// Every field is optional so the set can grow additively as each is wired to
/// an engine; a `None` means "the core keeps its own default", never zero.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlProps {
    /// Ramp applied at run start and at every level change, seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_ramp_s: Option<f64>,
    /// Controlled-stop ramp, seconds (NOT the abort taper).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shutdown_ramp_s: Option<f64>,
    /// Maximum singular values inverted per frequency line. The law's trust
    /// threshold remains the FLOOR (which directions are worth correcting);
    /// this is the CEILING on how many are used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_dof: Option<usize>,
    /// Correction damping, 0 (correct fully every loop) to 10 (spread a
    /// correction across ~10 loops).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damping: Option<f64>,
    /// Statistical degrees of freedom the CONTROL estimate targets - the
    /// industry-standard way to state random-control smoothing (2026-08-03).
    /// The service picks its accumulator depth so `2 x nAvg x windows` reaches
    /// this. Higher = a steadier estimate and a smoother trace, at the cost of
    /// slower adaptation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_dof: Option<f64>,
    /// Soft-start depth, dB below the LOWEST scheduled level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_start_db: Option<f64>,
    /// Envelope relaxation (dB) while the soft start is still climbing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_start_relax_db: Option<f64>,
    /// Lines allowed out of tolerance in one loop before it counts as diverging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_lines_out: Option<usize>,
    /// Consecutive diverging loops tolerated before abort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_loops_out: Option<u32>,
    /// Drive peak/RMS clip, in sigma.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_sigma: Option<f64>,
    /// Absolute drive ceiling, volts RMS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_drive_vrms: Option<f64>,
    /// Hold after sysID/coherence for operator review. `false` = let it
    /// ride; the automatic safety checks run either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_after_sysid: Option<bool>,
    // ---- 2026-08-03 additions -----------------------------------------------
    // Before these, the Control tab's Hinv card, the coherence floor and the
    // pretest knobs were edited in the UI and silently DEFAULTED on the
    // remote path. All optional: absent = the core keeps its own default.
    /// Band-coherence floor for the sysID gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coherence_floor: Option<f64>,
    /// Per-bin trust gate: coherence floor below which a bin's trim is held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_coherence_floor: Option<f64>,
    /// Per-bin trust gate: |H| floor as a fraction of the in-band peak.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_h_floor_frac: Option<f64>,
    /// Maximum per-bin shape-trim step, dB per control loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_correction_db_per_iter: Option<f64>,
    /// Tikhonov epsilon for the H-inverse seed; absent = the suggest policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lambda: Option<f64>,
    /// Consecutive same-direction clamps before a bin counts as divergent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub divergence_n: Option<u32>,
    /// sysID excitation level, dB relative to the pretest base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sysid_level_db: Option<f64>,
    /// Minimum pretest segments for the sysID estimate (the FRF-averages knob;
    /// the core's minimum-record floor still extends a short configuration).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pretest_frf_averages: Option<usize>,
    /// Pretest drive ceiling, volts RMS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pretest_drive_v: Option<f64>,
    /// Pretest drive floor, volts RMS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_pretest_drive_v: Option<f64>,
    /// Shape the pretest PSD like the reference instead of flat-in-band.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shaped_pretest: Option<bool>,
}

/// One reference-PSD breakpoint row (mirrors the `eigen-breakpoints` row vocabulary:
/// frequency, PSD-anchor-or-slope, optional per-row bands).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PsdBreakpoint {
    /// Breakpoint frequency, Hz.
    pub freq_hz: f64,
    /// The level at this row.
    pub level: PsdLevel,
    /// Per-row alarm band `(minus_db, plus_db)`; `None` = the profile default.
    pub alarm_db: Option<(f64, f64)>,
    /// Per-row abort band `(minus_db, plus_db)`; `None` = the profile default.
    pub abort_db: Option<(f64, f64)>,
}

/// How a breakpoint row states its level.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PsdLevel {
    /// A PSD anchor, g^2/Hz.
    Psd(f64),
    /// A slope from the prior anchor, dB/octave.
    SlopeDbPerOct(f64),
}

/// One level-schedule entry.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScheduleEntry {
    /// Level, dB relative to full.
    pub db: f64,
    /// Dwell at this level, seconds (`None` = until advanced).
    pub dwell_s: Option<f64>,
}

/// Whether the level ladder advances automatically or under operator control.
/// (The old surface exposed this as two separate run verbs; here the run verb is
/// [`Command::Start`] and auto-vs-manual is test DATA, where it belongs.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LadderMode {
    /// Dwell timers advance the schedule.
    Auto,
    /// Operator level commands advance the schedule.
    Manual,
}
