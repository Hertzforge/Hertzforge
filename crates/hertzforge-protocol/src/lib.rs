//! `hertzforge-protocol` - the versioned UI <-> core protocol seam (Layer 4/5).
//!
//! This crate reserves the network seam for the split deployment where the front end
//! (e.g. a Mac running the Svelte UI) and the core (e.g. a Linux box holding the
//! hardware) are different processes on different machines. Today both live in one
//! Tauri process; the seam exists so that separation is a transport change, not a
//! redesign. One dispatcher ([`dispatch`] over [`CoreApi`]), two transports: the
//! in-process Tauri command layer now, a socket server later.
//!
//! # CRITICAL RULE: the control loop stays entirely on the hardware host
//!
//! The network carries ONLY the UI/display path: decimated display frames down,
//! operator commands up. It NEVER carries the per-tick control loop - not drive
//! samples, not per-block feedback, not servo state. Closing a real-time servo loop
//! across a network link would destroy loop stability (unbounded, jittery transport
//! delay inside the loop) and blow the abort-latency budget that gates `arm()`.
//! Whatever closes the loop - the device's own DSP (a delegated controller) or Hertzforge's
//! engine on the host (NI / Sim) - is co-resident with the hardware, on the hardware host,
//! always.
//!
//! Corollary (safety): a [`Command::Stop`] or
//! [`Command::Abort`] arriving over this seam is an OPERATOR CONVENIENCE on the
//! supervisory path. It is never the primary abort. The primary abort authority is the
//! device's own on-hardware limits/interlock (delegated loop) or the host watchdog
//! co-resident with the drive clock (host loop), backed by the physical E-stop. No
//! remote UI, and no transport, is ever in the armature-safety chain.
//!
//! This is the Layer 4/5 (UI <-> core) seam, NOT a hardware seam. It must never
//! contain hardware wire encoding: no USB block layouts, no run-control words, no
//! templates, no checksums, no register maps. Those live behind the HAL on the core
//! side and do not cross this boundary.
//!
//! # Commands express operator INTENT, not device mechanics
//!
//! Hertzforge drives two fundamentally different hardware kinds. A delegated controller closes its
//! own loop and offers no absolute setters - the OEM surface is a step/toggle nudge
//! interface on a paced (~200 ms) cadence. An NI DAQ is the opposite: the host
//! synthesizes samples, absolute setpoints are natural, nudges are meaningless. A
//! protocol shaped like either device would bake that device's semantics into the
//! seam forever. So every verb here is the operator's intent in engineering units:
//!
//! - YES: [`Command::SetLevel`], [`Command::GoToFrequency`], [`Command::SetProfile`],
//!   [`Command::Start`], [`Command::Stop`], [`Command::RampDown`].
//! - NO: `SendNudges { count, direction }`, `StepFrequency { coarse | fine }`, or
//!   anything naming a wire word, block, template, or checksum.
//!
//! The core-side delegate service owns the intent -> mechanics translation. For a
//! delegated controller that translation already exists in the Explorer service's pacer: a
//! `frequency_jump` converger and `drive_jump` turn an absolute target into a paced
//! nudge sequence and converge on it. This protocol speaks the target; the pacer
//! speaks the nudges; the two never trade places.
//!
//! # Converging intents
//!
//! The same intent can be instant on one backend and take seconds on another.
//! `GoToFrequency { hz }` is a setpoint write on NI but a paced, re-measuring nudge
//! convergence on a delegated controller. A [`Response`] therefore distinguishes
//! [`Completion::Immediate`] from [`Completion::Converging`], and a converging intent
//! streams [`Event::IntentProgress`] until an [`Event::IntentSettled`] reports how it
//! ended: converged, clamped at a safety band edge, superseded by a newer intent,
//! aborted, out of budget, or failed. A UI that treats every accepted command as
//! instantaneously done will look broken on the delegated path; this type shape makes that
//! mistake hard to write.
//!
//! # Capability honesty
//!
//! [`CapabilityDescriptor`] tells the UI what a backend can actually do, in the HAL's
//! own vocabulary (loop owner, clock domain, native modes, abort-latency bound) and
//! per-action terms (unsupported / instant / converging). Modes are a CLOSED enum with
//! an exhaustive per-mode support matrix whose only convenience constructor claims
//! nothing - there is no string list to pad with fiction. A real implementation must
//! derive the matrix from the backend's `hertzforge_hal::Caps` (and the module registry for
//! hosted modes), never hand-write it. See [`caps`] for the full rules, including the
//! structural lies [`CapabilityDescriptor::inconsistencies`] detects mechanically.
//!
//! # Version rule: incompatible peers fail loudly and early, never silently misparse
//!
//! Every envelope carries [`PROTOCOL_VERSION`] on the wire, and peers exchange a
//! [`Hello`] before anything else. The policy is EXACT MATCH: any mismatch is refused
//! with [`ProtocolError::VersionMismatch`] naming both versions, at handshake time if
//! possible and at [`dispatch`] time regardless. There is no silent best-effort
//! parsing of a foreign version - a misparsed operator command aimed at a shaker is a
//! worse outcome than a refused connection. See [`envelope`] for the full rule.
//!
//! **This is also why the selectors precede the daemon**: with
//! an exact-match rule there is no graceful degradation across versions, so every wire
//! change has to land BEFORE the deployment that would have to negotiate it. Wire
//! changes are therefore batched into as few version bumps as possible - bump #1 was
//! v2, bump #2 was v3 ([`subscribe`] selectors, [`docs`] schema versions, the
//! [`session`] run lease + shared token, the capture LIST verb, and the channel-table
//! get/put verbs), bump #3 is v4, the Explorer-on-NI surface ([`ExplorerProfile`], the
//! [`ExplorerCommand`] group, and the [`StatusReport`] drive/sweep readouts), and
//! bump #4 is v5 - THREE modules at once ([`sine`], [`shock`]'s classical and SRS
//! halves) because two sessions were building against this seam simultaneously and
//! three separate bumps would have meant three coordinated redeploys - and bump #5 is
//! v6, the [`twr`] surface: one module, because it was the only one waiting, plus the
//! first new [`TestMode`] variant this crate has taken.
//!
//! The bump is also why [`SineProfile`] did NOT grow: it is `deny_unknown_fields`, so
//! a new field on it is a mid-workflow decode failure on an older peer rather than an
//! additive change. A new profile VARIANT is refused at connect instead, which is the
//! failure this crate wants.
//!
//! # One Controller, N Observers (v3)
//!
//! A hardware host serves exactly one peer that may DRIVE and any number that may
//! WATCH. [`Command::is_mutating`] is the single exhaustive statement of which is
//! which, and [`session`] carries the lease, the deliberate refuse-by-default answer
//! to a second Controller, and the client-vanish rule (the run CONTINUES; a
//! controller-timeout later brings it down through the controlled ramp, never an
//! abort). The lease decides who may ASK. It is not a safety mechanism and must never
//! be presented as one - the hardware decides what is safe.
//!
//! The [`Hello`] additionally carries a [`CoreStamp`], which is **compared and
//! reported, never enforced**: two dev machines, and the Mac-UI/Linux-core split
//! itself, legitimately differ. See [`envelope`] for that rule and
//! `PROTOCOL_CHANGELOG.md` for the version history.
//!
//! # Binary display frames cross as opaque bytes
//!
//! The scope / waveform / spectrum display frames are hand-packed little-endian byte
//! layouts, pinned by `ui/IPC_CONTRACT.md` and produced by explicit byte writers
//! (deliberately not `#[repr(C)]` transmutes) precisely so the layout is the contract
//! on every host. They are therefore already transport-agnostic. This crate carries
//! them as [`FrameBytes`] - an opaque byte payload tagged with a [`StreamKind`] -
//! WITHOUT re-encoding, re-packing, or interpreting them. Producer packers and the TS
//! decoder remain the two ends of that contract; the protocol is a pipe.
//!
//! # Dependency discipline (load-bearing)
//!
//! This crate depends on `serde` and nothing else. NO `tauri` (the UI-side peer of a
//! split deployment is not a Tauri process), NO `hertzforge-hal` or backend crates (the
//! UI-side peer must not link hardware code, and the HAL trait is allowed to evolve
//! without moving this wire contract). HAL vocabulary is mirrored into this crate's
//! own serializable types (`LoopOwner`, `ClockDomain`, `TestMode`, abort-latency) and
//! kept aligned by review, not by linkage - that is the price of a stable seam, paid
//! knowingly. Enum matches here are exhaustive (no `#[non_exhaustive]`): a core that
//! gains a verb must consciously handle it everywhere, and the version gate - not
//! permissive parsing - is the evolution mechanism.
//!
//! # How a transport adopts this
//!
//! Implement [`CoreApi`] over the real core state, then feed it envelopes:
//! Tauri commands deserialize their arguments, wrap them in a [`CommandEnvelope`],
//! call [`dispatch`], and emit [`Event`]s through an [`EventSink`] backed by the Tauri
//! event channel; a socket server does exactly the same with a length-delimited codec
//! on a TCP stream. Neither transport interprets payloads; both share the one
//! dispatcher, which is the point.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod api;
pub mod caps;
pub mod command;
pub mod docs;
pub mod envelope;
pub mod error;
pub mod event;
pub mod frames;
pub mod response;
pub mod session;
pub mod shock;
pub mod sine;
pub mod subscribe;
pub mod twr;

pub use api::{dispatch, CoreApi, EventSink, VecSink};
pub use caps::{
    ActionKind, ActionMatrix, ActionSupport, AdaptationOwner, BackendId, BackendInfo, BackendTier,
    CapabilityDescriptor, CapsInconsistency, ClockDomain, DriveOrigin, LinkState, LoopOwner,
    ModeMatrix, ModeSupport, Resolution, StationClass, TestMode, UnsupportedReason,
};
pub use command::{
    AcquisitionProps, Command, ControlProps, ExplorerCommand, ExplorerProfile, ExplorerSlew,
    FeasibilityInputs, FrfPairSpec, FrfSource, LadderMode, LevelTarget, LimitProfileRows, LimitRow,
    PsdBreakpoint, PsdLevel, RandomProfile, RecorderCommand, ScheduleEntry, SineProfile,
    SweepDirection, SweepSpec, TestProfile, FRF_PAIR_BASE, MAX_FRF_PAIRS,
};
pub use docs::{
    CaptureInfo, Doc, SchemaKind, TableDoc, CHANNEL_TABLE_SCHEMA, PROFILE_SCHEMA,
    PROVENANCE_SCHEMA, RECORDER_CONFIG_SCHEMA, SHOCK_NOTCH_SCHEMA, SHOCK_PULSE_SCHEMA,
    SHOCK_SRS_GRID_SCHEMA, TWR_REFERENCE_SCHEMA,
};
pub use envelope::{
    CommandEnvelope, CoreStamp, Envelope, EventEnvelope, Hello, HelloAck, PeerRole,
    ResponseEnvelope, StampComparison, VersionHeader, PROTOCOL_VERSION,
};
pub use error::ProtocolError;
pub use event::{Basis, EngineeringUnit, Event, IntentOutcome, Progress};
pub use frames::{FrameBytes, StreamKind};
pub use response::{ActiveIntent, Completion, IntentId, Response, RunState, StatusReport};
pub use session::{LeaseAnswer, LeaseId, ObserverReason, RunRole};
pub use shock::{
    ClassicalShockProfile, ShockCommand, ShockCompensation, ShockCompensationShape,
    ShockDriveLimit, ShockDriveMonitor, ShockGunfireSpec, ShockLevelSchedule, ShockMetaField,
    ShockMonitorKind, ShockPulseShape, ShockPulseSpec, ShockRateModel, ShockRunMode, ShockSpacing,
    ShockToleranceStandard, ShockTrainSpec, SrsBandFraction, SrsCommand, SrsEngineKind,
    SrsEnvelopeSpec, SrsFeasibilityTol, SrsFrfSpec, SrsKinematicAbort, SrsLimitChannel,
    SrsLimitSet, SrsLimitTarget, SrsProfile, SrsSynthesisOpts, SrsSysIdExcitation, SrsSysIdSpec,
    SrsTarget, SrsToleranceBand, DEFAULT_SHOCK_MAX_DRIVE_VPK,
};
pub use sine::{
    SineBandwidthMode, SineCaptureConfig, SineCaptureRule, SineColaConfig, SineColaWaveform,
    SineCommand, SineCompositeMode, SineCompression, SineControlChannel, SineControlProfile,
    SineControlSet, SineDwellFor, SineDwellPlan, SineFeedforwardConfig, SineLadderConfig,
    SineLimitProfile, SineMetaField, SineModeSpec, SineNotchConfig, SineProfileRow, SineQuantity,
    SineRstdDwellMode, SineRstdSpec, SineRungFor, SineStepSpacing, SineStepSpec, SineSweepRate,
    SineSweepSpec, SineSweepTarget, SineWireProfile,
};
pub use subscribe::{ChannelSel, StreamSub};
pub use twr::{
    TwrCommand, TwrExcitation, TwrFeasibilityCeilings, TwrGateFailPolicy, TwrInverseSpec,
    TwrLawSpec, TwrLevelSchedule, TwrLimitsSpec, TwrMetaField, TwrPrep, TwrProfile, TwrRunMode,
    TwrRunSchedule, TwrSection, TwrSysIdSpec, DEFAULT_TWR_MAX_DRIVE_VPK,
};
