//! The capability descriptor: what a backend can HONESTLY do, in the UI's terms.
//!
//! # Vocabulary is mirrored from `eigen-hal`, never imported
//!
//! [`LoopOwner`], [`ClockDomain`], [`TestMode`] and the abort-latency field mirror
//! `eigen_hal::Caps` (`loop_owner`, `clock`, `native_modes`, `abort_latency_bound`)
//! one-for-one, as this crate's own serializable types. This crate must not depend on
//! `eigen-hal` (the UI-side peer of a split deployment never links hardware code, and
//! the HAL trait is allowed to evolve without moving the wire contract), so alignment
//! is kept by review against `crates/eigen-hal/src/lib.rs`, deliberately.
//!
//! # Designed so a fictional second source of truth is hard to reintroduce
//!
//! The pre-seam UI kept a hand-written capability table that claimed the delegated
//! controller natively runs five modes when the real backend declares ONE native mode
//! (`OpenLoopSine`), under a comment asserting the values came from the real backend.
//! Free-form string lists made that lie cheap. This module removes the affordances
//! that allowed it:
//!
//! - [`TestMode`] is a CLOSED enum mirroring the HAL's native-mode vocabulary - there
//!   is no string to invent (the old table's `"explorer"` / `"recorder"` entries are
//!   inexpressible here; they are UI surfaces, not control modes).
//! - [`ModeMatrix`] is an EXHAUSTIVE struct, one field per mode: you must state a
//!   posture for every mode, and the only convenience constructor
//!   ([`ModeMatrix::all_unsupported`], also `Default`) claims NOTHING. Support must be
//!   asserted mode-by-mode, explicitly, in code a reviewer can point at.
//! - [`ModeSupport`] forces the author to say, for each supported mode, WHO produces
//!   the drive samples and WHO computes corrections ([`DriveOrigin`] x
//!   [`AdaptationOwner`]), which makes the claim mechanically cross-checkable against
//!   [`CapabilityDescriptor::loop_owner`] and [`CapabilityDescriptor::clock`]:
//!   [`CapabilityDescriptor::inconsistencies`] flags the structural lies (host-
//!   synthesized drive on a device-owned loop, device replay on a host-owned loop,
//!   per-tick host adaptation without a proven-`Shared` clock, correction with no
//!   drive to correct).
//! - An UNSUPPORTED mode carries a TYPED [`UnsupportedReason`], not a bare "no". The
//!   operator has to know WHICH precondition failed in order to act on it, and a UI
//!   cannot branch on prose.
//!
//! What validation CANNOT catch is a device-replay claim for a native mode the device
//! does not actually have - only the backend knows its `native_modes`. That is why
//! the sourcing rule below is part of the contract, not just advice.
//!
//! # The `NativeMode` mirror has ONE stated exception: [`TestMode::Twr`] (v6)
//!
//! Every other [`TestMode`] variant has a counterpart in `eigen_hal::NativeMode`,
//! because every other one names something a device could plausibly run on its own.
//! TWR does not, and `NativeMode` deliberately will NOT grow a `Twr`:
//!
//! - TWR's law runs BETWEEN plays, on the host, over seconds. Even on a smart
//!   controller that replays a downloaded record, the ITERATION is host-side -
//!   `{DeviceReplay, HostBetweenRuns}`, the cell the two-axis shape exists to name (see
//!   [`ModeSupport`]'s docs, which already cite TWR as the case that proves it).
//! - So there is no "the device natively runs TWR" claim for a backend to make, and a
//!   `NativeMode::Twr` would be a slot whose only possible value is a lie.
//!
//! The consequence for the sourcing rule above: `modes.twr` can never be filled by the
//! `Caps::native_modes` half of the constructor. It is filled from the resolver like
//! any other hosted mode, and a CLAIM of support is a MANUAL [`ModeMatrix`] assignment
//! in the core, made after the module's on-chassis gate is a recorded PASS and citing
//! that ledger entry - the Shock precedent. It is never a `native_modes` proof, because
//! there is no native mode to prove. The evidence-first rule is unchanged by the
//! exception: the evidence still precedes the claim, only the evidence is a bench
//! ledger rather than a device's own declaration.
//!
//! # How a real implementation must source this (the single-source rule)
//!
//! The core-side server builds the descriptor FROM the live `eigen_hal::Caps` of the
//! active backend - a `From<&Caps>`-style constructor in the server crate (which may
//! depend on `eigen-hal`; this crate must not):
//!
//! - `loop_owner`, `clock`, channel counts, sample rate, `abort_latency_bound`:
//!   copied field-for-field from `Caps`.
//! - Each `Caps::native_modes` entry becomes [`ModeSupport::delegated`] for the
//!   matching [`TestMode`].
//! - Each module the host can run against this backend gets the [`Resolution`] the
//!   amendment's resolver returned for it - [`ModeSupport::hosted`] for the
//!   continuous per-tick shape, and the iterative / open-loop / capture-only shapes
//!   spelled out explicitly rather than collapsed into it.
//! - Everything else becomes [`ModeSupport::Unsupported`] carrying the resolver's own
//!   typed reason - a real, displayable, ACTIONABLE answer the UI uses to refuse
//!   honestly, not an error.
//!
//! Hand-writing a descriptor literal for real hardware is what this design exists to
//! prevent; the ONE hand-written descriptor allowed is in a test, constructing the
//! old UI fiction to prove [`CapabilityDescriptor::inconsistencies`] rejects it.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

/// Who closes the control loop on this backend. Mirrors `eigen_hal::LoopOwner`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopOwner {
    /// The device runs its own control DSP (a delegated controller); Eigen delegates + observes.
    DeviceClosesLoop,
    /// Eigen's engine runs the control math on the hardware host (e.g. NI, Sim).
    HostClosesLoop,
}

/// Whether input (measurement) and output (drive) share one physical clock. Mirrors
/// `eigen_hal::ClockDomain`, including the honest `Unknown` default: a backend that
/// has not PROVEN `Shared` by bench loopback must not claim it, and the engine
/// refuses closed-loop modes until it is `Shared`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockDomain {
    /// Proven same-clock: closed-loop modes may arm.
    Shared,
    /// Separate clocks: closed-loop modes are refused.
    Independent,
    /// Not yet characterized (the default posture): closed-loop modes are refused.
    Unknown,
}

/// A control/test mode, in the HAL's native-mode vocabulary (mirrors
/// `eigen_hal::NativeMode`, with the ONE stated exception the module docs name).
/// Closed enum on purpose: capability claims can only be made about modes this seam
/// actually models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestMode {
    /// Open-loop sine (the SMART/Explorer surface): commanded drive, measured response.
    OpenLoopSine,
    /// Closed-loop swept sine (tracking filter + amplitude servo).
    Sine,
    /// Closed-loop random (spectral control to a PSD reference).
    Random,
    /// Classical shock pulses.
    Shock,
    /// Shock response spectrum synthesis.
    Srs,
    /// Time waveform replication (v6): a recorded transient replicated by iterating a
    /// drive between plays.
    ///
    /// **The mirror exception.** `eigen_hal::NativeMode` has no `Twr` and deliberately
    /// will not get one - see the module docs. TWR is a host-only mode on every path,
    /// so a claim of support here is a manual [`ModeMatrix`] assignment citing a bench
    /// ledger, never a `native_modes` proof.
    Twr,
    /// Multi-input multi-output control.
    Mimo,
}

impl TestMode {
    /// True for the modes whose HOSTED implementation closes a real-time loop on the
    /// host (Random = closed-loop spectral, Sine = narrowband tracking, MIMO =
    /// multi-drive control; SMART open-loop sine, Shock, and SRS are
    /// open-loop synthesis).
    ///
    /// **No longer used by [`CapabilityDescriptor::inconsistencies`]** as of v2. The
    /// shared-clock requirement binds [`AdaptationOwner::HostPerTick`] - the actual
    /// precondition - rather than this per-mode list, which was a proxy for it and
    /// wrong in both directions: it demanded a clock for a mode the host merely
    /// SYNTHESIZES open-loop, and could not demand one for a per-tick-corrected mode
    /// outside the list. Kept because it is a true statement about the modes'
    /// canonical hosted implementations, and a UI may want it for labelling.
    ///
    /// **[`TestMode::Twr`] answers `false`, and that is a decision rather than an
    /// omission (v6).** TWR's loop is real, but it closes BETWEEN plays over seconds -
    /// [`AdaptationOwner::HostBetweenRuns`], not [`AdaptationOwner::HostPerTick`].
    /// Listing it here would be this method's own documented v1 conflation in a new
    /// place: the shared-clock precondition binds per-tick correction, and demanding a
    /// proven clock for a mode that plays a record and then thinks about it would
    /// refuse a run that is safe without one.
    pub fn closes_loop_on_host(self) -> bool {
        matches!(self, TestMode::Sine | TestMode::Random | TestMode::Mimo)
    }

    /// All modes, for exhaustive iteration (kept in one place so it cannot drift from
    /// the enum).
    pub const ALL: [TestMode; 7] = [
        TestMode::OpenLoopSine,
        TestMode::Sine,
        TestMode::Random,
        TestMode::Shock,
        TestMode::Srs,
        TestMode::Twr,
        TestMode::Mimo,
    ];
}

/// Axis 1 of a loop resolution: WHO PRODUCES THE DRIVE SAMPLES?
///
/// Mirrors `eigen_module_abi::DriveOrigin` (see the module docs: mirrored, never
/// imported - `eigen-module-abi` is a hardware-side crate and this seam must not link
/// it). Kept field-for-field so a reviewer can diff the two enums.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveOrigin {
    /// No drive path at all. The Recorder's shape: capture only.
    None,
    /// Eigen computes and streams every sample to the device.
    HostSynthesized,
    /// Eigen downloads a reference/config and the DEVICE plays it from its own memory.
    /// Carries WHICH native mode, so the UI can name what was resolved.
    DeviceReplay {
        /// The device-native mode being replayed.
        mode: TestMode,
    },
}

/// Axis 2 of a loop resolution: WHO COMPUTES CORRECTIONS, AND ON WHAT TIMESCALE?
///
/// Mirrors `eigen_module_abi::AdaptationOwner`. This is the axis the three-valued
/// [`ModeSupport`] was missing: for CONTINUOUS modules the two axes coincide (whoever
/// synthesizes the samples necessarily corrects them, because the correction IS the
/// next sample), which is why the conflation went unnoticed - the original enum was
/// written from Sine and Random. For TRANSIENT / ITERATIVE modules they separate: the
/// loop closes over seconds (play -> measure -> correct -> play), so "who plays" and
/// "who corrects" are genuinely independent. TWR is the case that proves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdaptationOwner {
    /// Open loop: no correction at all. SMART; TWR scheduled playback.
    None,
    /// Eigen corrects INSIDE the run, per tick. Random COLA, the Sine servo.
    HostPerTick,
    /// Eigen corrects BETWEEN plays/shots. TWR iteration, SRS wavelets.
    HostBetweenRuns,
    /// The device's own control loop corrects. Random/Shock on a delegated controller.
    Device,
}

/// One resolved loop assignment: both axes together. Mirrors
/// `eigen_module_abi::Resolution`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    /// Who produces the drive samples.
    pub drive: DriveOrigin,
    /// Who computes corrections, and on what timescale.
    pub adaptation: AdaptationOwner,
}

impl Resolution {
    /// The common constructor.
    pub const fn new(drive: DriveOrigin, adaptation: AdaptationOwner) -> Self {
        Resolution { drive, adaptation }
    }

    /// Is there any drive path at all? (`false` only for the Recorder shape.)
    pub fn has_drive(&self) -> bool {
        !matches!(self.drive, DriveOrigin::None)
    }

    /// Does the DEVICE produce the drive samples?
    pub fn is_delegated_drive(&self) -> bool {
        matches!(self.drive, DriveOrigin::DeviceReplay { .. })
    }

    /// Does this resolution close a per-tick loop on the host? The condition that
    /// requires a proven-[`ClockDomain::Shared`] clock.
    pub fn is_host_per_tick(&self) -> bool {
        matches!(self.adaptation, AdaptationOwner::HostPerTick)
    }
}

/// Why a requested loop assignment cannot be honored, mirrored from
/// `eigen_module_abi::UnsupportedReason`.
///
/// TYPED, not a string, for the same reason the ABI's is: the operator needs to know
/// WHICH precondition failed in order to act (plug in a chassis clock route vs. wait
/// for a telemetry gap to close vs. regenerate a profile), and a UI cannot branch on
/// prose. The variants that carry hardware detail carry it as this crate's own types.
///
/// `PartialEq` but NOT `Eq`: [`UnsupportedReason::RateMismatch`] carries `f64` rates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedReason {
    /// A device replay was requested for a mode the device does not PROVE
    /// (`requested: Some`), or delegation was requested with no mode named at all
    /// (`requested: None`). `Caps::native_modes` is an evidence claim, not an
    /// inventory: a delegate path that is fully BUILT in software stays refused until
    /// the device's caps list the mode.
    NoNativeMode {
        /// The mode that was asked for, if one was named.
        requested: Option<TestMode>,
        /// The modes the device actually proves.
        proven: Vec<TestMode>,
    },
    /// Per-tick host adaptation was requested against a clock domain that is not
    /// proven `Shared`.
    ClockNotShared {
        /// The clock posture the backend actually reports.
        clock: ClockDomain,
    },
    /// Host adaptation BETWEEN runs was requested over a device replay, but the
    /// measured response does not come back to the host, so the iteration has no error
    /// signal. The resolver REFUSES rather than silently degrading to open-loop replay.
    ResponseNotObservable {
        /// The device-native mode that would have been replayed.
        mode: TestMode,
    },
    /// A drive path was requested on a HARDWARE station with no abort-latency bound -
    /// measured or provisional. A station-class rule, not a loop-shape rule.
    NoAbortLatencyBound {
        /// The station class that has no bound.
        station: StationClass,
    },
    /// Delegation was requested of a backend that runs no control loop of its own (a
    /// DAQ card, the Sim plant). There is nothing to delegate TO.
    DelegationTargetIsHostLoop {
        /// The mode that was asked for, if one was named.
        requested: Option<TestMode>,
    },
    /// Host-synthesized drive was requested of a backend that generates its own drive.
    /// There is no per-sample host output path to insert control into, by design.
    NoHostDrivePath {
        /// The modes the device proves instead.
        proven: Vec<TestMode>,
    },
    /// Correction was requested for a request that has no drive path to correct. An
    /// incoherent pairing, named rather than silently dropped.
    NothingToAdapt {
        /// The correction owner that was asked for.
        adaptation: AdaptationOwner,
    },
    /// The profile was generated at one rate and the hardware is configured at
    /// another. **Eigen NEVER resamples to close this gap** - that would change the
    /// spectrum of the test the operator approved, silently, at arm time.
    RateMismatch {
        /// The rate the profile was generated at.
        profile_rate_hz: f64,
        /// The rate the hardware actually configured.
        actual_rate_hz: f64,
        /// `|profile - actual| / actual`.
        relative_error: f64,
        /// Within the "regenerate the profile" band (<= 1%) rather than the
        /// gross-mismatch band. Both refuse; the offer differs.
        regeneration_offered: bool,
    },
}

/// Which kind of station a run is arming against. Mirrors
/// `eigen_module_abi::StationClass`.
///
/// Not a backend property: `NiSimulated` and `NiReal` are the SAME backend
/// distinguished by what the DRIVER says about the devices, and `EigenSim` is a
/// different backend entirely. The UI badges them differently, which is why the
/// distinction has to cross this seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StationClass {
    /// The `eigen-hal-sim` physics plant. Control-law truth lives here; there is no
    /// hardware, no armature, and nothing to abort.
    EigenSim,
    /// The NI backend on driver-simulated devices (`DevIsSimulated`, amber badge).
    /// Real driver, real task machinery, no physical output.
    NiSimulated,
    /// The NI backend on physical hardware (serial badge).
    NiReal,
}

impl StationClass {
    /// Is this a HARDWARE station (`NI-SIM` union `NI-REAL`)? The abort-latency rule
    /// binds these and exempts `EigenSim`.
    pub fn is_hardware(&self) -> bool {
        matches!(self, StationClass::NiSimulated | StationClass::NiReal)
    }
}

/// How (and whether) a backend supports one [`TestMode`].
///
/// # v2: the two-axis shape (wire bump #1)
///
/// v1 was three-valued (`Hosted` / `Delegated` / `Unsupported`), mirroring the
/// original single-axis loop resolution. A 2026-07-16 design change split loop
/// resolution into TWO axes ([`DriveOrigin`] x [`AdaptationOwner`]) because
/// the single axis could not name the cells that actually exist - most sharply
/// `{DeviceReplay, HostBetweenRuns}` (the device plays, the host iterates between
/// plays: TWR on a smart controller) and `{DeviceReplay, None}` (frozen playback with
/// nothing correcting). Calling either of those "Delegated" told the UI the device was
/// closing a loop it is not closing.
///
/// [`ModeSupport::Supported`] therefore carries the full [`Resolution`], and
/// [`ModeSupport::Unsupported`] carries the TYPED [`UnsupportedReason`] rather than
/// being a bare "no" - the operator needs to know which precondition failed in order
/// to act on it. [`ModeSupport::hosted`] / [`ModeSupport::delegated`] reproduce the v1
/// shapes for the common cases, and
/// [`ModeSupport::is_hosted`] / [`ModeSupport::is_delegated`] answer the v1 questions,
/// so the migration is mechanical for consumers that only asked those.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeSupport {
    /// The mode runs, in exactly this resolution.
    Supported(Resolution),
    /// The mode does not run, for this reason. An honest, displayable answer - the UI
    /// shows the mode as unavailable WITH the precondition that failed, rather than
    /// pretending, and rather than saying only "no".
    Unsupported(UnsupportedReason),
}

impl ModeSupport {
    /// The v1 `Hosted` shape: the host synthesizes the drive and corrects per tick
    /// (`{HostSynthesized, HostPerTick}`) - the continuous host-loop cell.
    pub const fn hosted() -> Self {
        ModeSupport::Supported(Resolution::new(
            DriveOrigin::HostSynthesized,
            AdaptationOwner::HostPerTick,
        ))
    }

    /// The v1 `Delegated` shape: the device replays `mode` AND corrects
    /// (`{DeviceReplay(mode), Device}`) - full delegation.
    pub const fn delegated(mode: TestMode) -> Self {
        ModeSupport::Supported(Resolution::new(
            DriveOrigin::DeviceReplay { mode },
            AdaptationOwner::Device,
        ))
    }

    /// The claim-nothing default for a mode nobody has asserted support for. The
    /// reason is [`UnsupportedReason::NoNativeMode`] with nothing requested and
    /// nothing proven, which is literally true of an unasserted mode.
    pub fn unclaimed() -> Self {
        ModeSupport::Unsupported(UnsupportedReason::NoNativeMode {
            requested: None,
            proven: Vec::new(),
        })
    }

    /// Does the mode run at all?
    pub fn is_supported(&self) -> bool {
        matches!(self, ModeSupport::Supported(_))
    }

    /// The v1 question "is this Hosted?": the HOST produces the drive samples.
    pub fn is_hosted(&self) -> bool {
        matches!(
            self,
            ModeSupport::Supported(Resolution {
                drive: DriveOrigin::HostSynthesized,
                ..
            })
        )
    }

    /// The v1 question "is this Delegated?": the DEVICE produces the drive samples.
    pub fn is_delegated(&self) -> bool {
        matches!(
            self,
            ModeSupport::Supported(Resolution {
                drive: DriveOrigin::DeviceReplay { .. },
                ..
            })
        )
    }

    /// The resolution, when supported.
    pub fn resolution(&self) -> Option<Resolution> {
        match self {
            ModeSupport::Supported(r) => Some(*r),
            ModeSupport::Unsupported(_) => None,
        }
    }

    /// The refusal reason, when unsupported.
    pub fn reason(&self) -> Option<&UnsupportedReason> {
        match self {
            ModeSupport::Supported(_) => None,
            ModeSupport::Unsupported(r) => Some(r),
        }
    }
}

/// Per-mode support, one REQUIRED field per [`TestMode`]. Exhaustive struct on
/// purpose (see the module docs): there is no list to pad, and nothing is claimable
/// by default.
///
/// No longer `Copy` as of v2: [`ModeSupport`] now carries a typed refusal reason,
/// which carries `Vec<TestMode>` in two variants. That is the right trade - a reason
/// the operator can act on is worth more than a bitwise copy of a capability table
/// that is built once per backend selection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModeMatrix {
    /// Support for [`TestMode::OpenLoopSine`].
    pub open_loop_sine: ModeSupport,
    /// Support for [`TestMode::Sine`].
    pub sine: ModeSupport,
    /// Support for [`TestMode::Random`].
    pub random: ModeSupport,
    /// Support for [`TestMode::Shock`].
    pub shock: ModeSupport,
    /// Support for [`TestMode::Srs`].
    pub srs: ModeSupport,
    /// Support for [`TestMode::Twr`] (v6).
    ///
    /// The one row that can never be filled from `Caps::native_modes` - see the module
    /// docs' mirror exception. A CLAIM here is a manual assignment citing a bench
    /// ledger, made after the chassis gate is a recorded PASS.
    pub twr: ModeSupport,
    /// Support for [`TestMode::Mimo`].
    pub mimo: ModeSupport,
}

impl ModeMatrix {
    /// The claim-nothing baseline. Start here and assert support mode-by-mode from a
    /// real source (see the module docs' single-source rule).
    pub fn all_unsupported() -> Self {
        ModeMatrix {
            open_loop_sine: ModeSupport::unclaimed(),
            sine: ModeSupport::unclaimed(),
            random: ModeSupport::unclaimed(),
            shock: ModeSupport::unclaimed(),
            srs: ModeSupport::unclaimed(),
            twr: ModeSupport::unclaimed(),
            mimo: ModeSupport::unclaimed(),
        }
    }

    /// Look up one mode's support.
    pub fn support(&self, mode: TestMode) -> &ModeSupport {
        match mode {
            TestMode::OpenLoopSine => &self.open_loop_sine,
            TestMode::Sine => &self.sine,
            TestMode::Random => &self.random,
            TestMode::Shock => &self.shock,
            TestMode::Srs => &self.srs,
            TestMode::Twr => &self.twr,
            TestMode::Mimo => &self.mimo,
        }
    }

    /// Mutable access to one mode's slot, so a `From<&Caps>` constructor can fill the
    /// matrix by iterating [`TestMode::ALL`] instead of writing one assignment per mode.
    pub fn support_mut(&mut self, mode: TestMode) -> &mut ModeSupport {
        match mode {
            TestMode::OpenLoopSine => &mut self.open_loop_sine,
            TestMode::Sine => &mut self.sine,
            TestMode::Random => &mut self.random,
            TestMode::Shock => &mut self.shock,
            TestMode::Srs => &mut self.srs,
            TestMode::Twr => &mut self.twr,
            TestMode::Mimo => &mut self.mimo,
        }
    }
}

/// `Default` is the claim-nothing matrix - a descriptor built lazily can only
/// UNDER-promise, never over-promise.
impl Default for ModeMatrix {
    fn default() -> Self {
        ModeMatrix::all_unsupported()
    }
}

/// The hardware-touching protocol actions a backend can support, mirrored by
/// [`ActionMatrix`]. Transport-universal commands (status, capability queries,
/// subscriptions, backend listing/selection) are always available and deliberately
/// not matrixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    /// [`crate::Command::SetProfile`].
    SetProfile,
    /// [`crate::Command::Start`].
    Start,
    /// [`crate::Command::Stop`].
    Stop,
    /// [`crate::Command::Abort`].
    Abort,
    /// [`crate::Command::RampDown`].
    RampDown,
    /// [`crate::Command::SetLevel`].
    SetLevel,
    /// [`crate::Command::AdjustLevel`].
    AdjustLevel,
    /// [`crate::Command::GoToFrequency`].
    GoToFrequency,
    /// [`crate::Command::AdjustFrequency`].
    AdjustFrequency,
}

/// Whether an action is available, and if so whether it completes within the command
/// round-trip or converges over time. This is what lets the UI adapt honestly: an
/// `Instant` action can be a plain control; a `Converging` action needs in-flight
/// affordances (progress, supersede-on-retarget, cancel); an `Unsupported` action is
/// not drawn as available at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionSupport {
    /// The backend cannot perform this action; commands get
    /// [`crate::ProtocolError::UnsupportedAction`].
    Unsupported,
    /// Applied before the response returns ([`crate::Completion::Immediate`]) - the
    /// absolute-setpoint hardware kind (host-synthesized drive).
    Instant,
    /// Accepted, then converges over time with progress events
    /// ([`crate::Completion::Converging`]) - the paced/nudged hardware kind. Expect
    /// seconds, not milliseconds.
    Converging,
}

/// Per-action support, one REQUIRED field per [`ActionKind`] - the same
/// no-list-to-pad design as [`ModeMatrix`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionMatrix {
    /// Support for [`ActionKind::SetProfile`].
    pub set_profile: ActionSupport,
    /// Support for [`ActionKind::Start`].
    pub start: ActionSupport,
    /// Support for [`ActionKind::Stop`].
    pub stop: ActionSupport,
    /// Support for [`ActionKind::Abort`].
    pub abort: ActionSupport,
    /// Support for [`ActionKind::RampDown`].
    pub ramp_down: ActionSupport,
    /// Support for [`ActionKind::SetLevel`].
    pub set_level: ActionSupport,
    /// Support for [`ActionKind::AdjustLevel`].
    pub adjust_level: ActionSupport,
    /// Support for [`ActionKind::GoToFrequency`].
    pub go_to_frequency: ActionSupport,
    /// Support for [`ActionKind::AdjustFrequency`].
    pub adjust_frequency: ActionSupport,
}

impl ActionMatrix {
    /// The claim-nothing baseline.
    pub fn all_unsupported() -> Self {
        ActionMatrix {
            set_profile: ActionSupport::Unsupported,
            start: ActionSupport::Unsupported,
            stop: ActionSupport::Unsupported,
            abort: ActionSupport::Unsupported,
            ramp_down: ActionSupport::Unsupported,
            set_level: ActionSupport::Unsupported,
            adjust_level: ActionSupport::Unsupported,
            go_to_frequency: ActionSupport::Unsupported,
            adjust_frequency: ActionSupport::Unsupported,
        }
    }

    /// Look up one action's support.
    pub fn support(&self, action: ActionKind) -> ActionSupport {
        match action {
            ActionKind::SetProfile => self.set_profile,
            ActionKind::Start => self.start,
            ActionKind::Stop => self.stop,
            ActionKind::Abort => self.abort,
            ActionKind::RampDown => self.ramp_down,
            ActionKind::SetLevel => self.set_level,
            ActionKind::AdjustLevel => self.adjust_level,
            ActionKind::GoToFrequency => self.go_to_frequency,
            ActionKind::AdjustFrequency => self.adjust_frequency,
        }
    }
}

/// `Default` is the claim-nothing matrix.
impl Default for ActionMatrix {
    fn default() -> Self {
        ActionMatrix::all_unsupported()
    }
}

/// Opaque backend identity (an open set - new vendors are not a protocol change).
/// Identity is fine at this seam; MECHANICS are not.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BackendId(pub String);

impl BackendId {
    /// Convenience constructor.
    pub fn new(id: impl Into<String>) -> Self {
        BackendId(id.into())
    }
}

impl fmt::Display for BackendId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether a backend row is installed/selectable or a roadmap slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendTier {
    /// Installed and selectable.
    Available,
    /// A named roadmap slot; selecting it is refused honestly.
    Planned,
}

/// Physical-link posture for a backend row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    /// No physical link applies (e.g. the Sim).
    NotApplicable,
    /// Probe found the device present.
    Detected,
    /// Probe did not find the device.
    NotDetected,
}

/// One row of the backend registry (the station selector), for
/// [`crate::Command::ListBackends`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackendInfo {
    /// Stable identity, used by [`crate::Command::SelectBackend`].
    pub id: BackendId,
    /// Human-readable label.
    pub label: String,
    /// Vendor name.
    pub vendor: String,
    /// Installed vs. roadmap slot.
    pub tier: BackendTier,
    /// Whether this row is the current station selection.
    pub selected: bool,
    /// Physical-link probe result.
    pub link: LinkState,
    /// Full capability descriptor, when the backend can produce one (`None` for
    /// planned slots - absence of claims, not a claim).
    pub caps: Option<CapabilityDescriptor>,
}

/// What one backend can do, in UI-consumable terms. See the module docs for the
/// mirroring rules, the anti-fiction design, and the single-source rule for
/// populating it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    /// Which backend this describes.
    pub backend: BackendId,
    /// Who closes the control loop (mirrors `Caps::loop_owner`).
    pub loop_owner: LoopOwner,
    /// Clock-domain posture (mirrors `Caps::clock`).
    pub clock: ClockDomain,
    /// Input sample rate in Hz (mirrors `Caps::sample_rate_hz`).
    pub sample_rate_hz: f64,
    /// Maximum input (measurement) channels (mirrors `Caps::max_input_channels`).
    pub max_input_channels: u32,
    /// Maximum output (drive) channels (mirrors `Caps::max_output_channels`).
    pub max_output_channels: u32,
    /// Worst-case guaranteed abort latency, if characterized (mirrors
    /// `Caps::abort_latency_bound`). `None` = not characterized: treat the physical
    /// E-stop as the safety boundary, and say so in the UI.
    pub abort_latency_bound: Option<Duration>,
    /// Per-mode support (exhaustive; see [`ModeMatrix`]).
    pub modes: ModeMatrix,
    /// Per-action support (exhaustive; see [`ActionMatrix`]).
    pub actions: ActionMatrix,
    /// Which display-frame streams this backend/core can deliver.
    pub streams: Vec<crate::frames::StreamKind>,
}

/// A structural lie [`CapabilityDescriptor::inconsistencies`] can detect
/// mechanically. Each variant names the mode so the report is actionable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapsInconsistency {
    /// A mode claims [`ModeSupport::Hosted`] but the loop owner is the device - the
    /// host cannot insert its control math between a device's loop and its DAC.
    HostedModeOnDeviceOwnedLoop {
        /// The offending mode.
        mode: TestMode,
    },
    /// A mode claims [`ModeSupport::Delegated`] but the loop owner is the host -
    /// there is no device-native loop to delegate to.
    DelegatedModeOnHostOwnedLoop {
        /// The offending mode.
        mode: TestMode,
    },
    /// A mode claims per-tick HOST adaptation ([`AdaptationOwner::HostPerTick`])
    /// without a proven-`Shared` clock; the HAL refuses to arm that on an unproven
    /// clock, so advertising it would promise what `arm()` will refuse.
    HostedClosedLoopWithoutSharedClock {
        /// The offending mode.
        mode: TestMode,
    },
    /// A mode claims a correction owner but no drive path - something to correct with
    /// nothing to correct. Inexpressible under the v1 single-axis enum; checkable now
    /// that the two axes are separate.
    AdaptationWithoutDrive {
        /// The offending mode.
        mode: TestMode,
    },
}

impl fmt::Display for CapsInconsistency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapsInconsistency::HostedModeOnDeviceOwnedLoop { mode } => write!(
                f,
                "{mode:?} claims Hosted but the device owns the loop - fiction; source the \
                 matrix from the backend's Caps"
            ),
            CapsInconsistency::DelegatedModeOnHostOwnedLoop { mode } => write!(
                f,
                "{mode:?} claims Delegated but the host owns the loop - there is no native \
                 mode to delegate to"
            ),
            CapsInconsistency::HostedClosedLoopWithoutSharedClock { mode } => write!(
                f,
                "{mode:?} claims per-tick host adaptation without a proven-Shared clock - \
                 arm() will refuse what this advertises"
            ),
            CapsInconsistency::AdaptationWithoutDrive { mode } => write!(
                f,
                "{mode:?} claims a correction owner but no drive path - there is nothing \
                 for it to correct"
            ),
        }
    }
}

impl CapabilityDescriptor {
    /// A truthful-by-construction baseline for `backend`: claims no modes, no
    /// actions, no streams, an `Unknown` clock, and an uncharacterized abort bound.
    /// Real support is then asserted field-by-field from the live `Caps` (see the
    /// module docs). Chooses `HostClosesLoop` arbitrarily; set `loop_owner` from the
    /// real source like everything else.
    pub fn claim_nothing(backend: BackendId) -> Self {
        CapabilityDescriptor {
            backend,
            loop_owner: LoopOwner::HostClosesLoop,
            clock: ClockDomain::Unknown,
            sample_rate_hz: 0.0,
            max_input_channels: 0,
            max_output_channels: 0,
            abort_latency_bound: None,
            modes: ModeMatrix::all_unsupported(),
            actions: ActionMatrix::all_unsupported(),
            streams: Vec::new(),
        }
    }

    /// Detect the structural lies this type can check without the hardware (see
    /// [`CapsInconsistency`]). Empty = structurally consistent. A server SHOULD
    /// refuse to advertise a descriptor that reports inconsistencies - it means the
    /// descriptor was hand-written rather than sourced from `Caps`.
    pub fn inconsistencies(&self) -> Vec<CapsInconsistency> {
        let mut found = Vec::new();
        for mode in TestMode::ALL {
            let Some(res) = self.modes.support(mode).resolution() else {
                continue;
            };
            match res.drive {
                DriveOrigin::HostSynthesized => {
                    if self.loop_owner == LoopOwner::DeviceClosesLoop {
                        found.push(CapsInconsistency::HostedModeOnDeviceOwnedLoop { mode });
                    }
                }
                DriveOrigin::DeviceReplay { .. } => {
                    if self.loop_owner == LoopOwner::HostClosesLoop {
                        found.push(CapsInconsistency::DelegatedModeOnHostOwnedLoop { mode });
                    }
                }
                DriveOrigin::None => {}
            }
            // The clock rule binds the ADAPTATION axis, not the drive axis - and that
            // is the correction the two-axis shape buys. Under v1 this test read
            // "hosted AND closes_loop_on_host", which conflated per-tick correction
            // with host synthesis: it demanded a Shared clock for a mode the host
            // merely SYNTHESIZES open-loop, and could not demand one at all for a
            // per-tick-corrected mode that did not happen to be Sine/Random/MIMO.
            // Only per-tick correction needs drive and response on one clock.
            if res.is_host_per_tick() && self.clock != ClockDomain::Shared {
                found.push(CapsInconsistency::HostedClosedLoopWithoutSharedClock { mode });
            }
            // A correction owner with no drive to correct is incoherent, and the
            // two-axis shape is the first version that can express it - so it is the
            // first version that has to check it.
            if !res.has_drive() && !matches!(res.adaptation, AdaptationOwner::None) {
                found.push(CapsInconsistency::AdaptationWithoutDrive { mode });
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_claim_nothing() {
        // The baseline can only under-promise: every mode and action unsupported.
        let m = ModeMatrix::default();
        for mode in TestMode::ALL {
            assert!(!m.support(mode).is_supported());
            // And it says WHY, rather than being a bare "no": nothing was requested
            // and nothing is proven, which is literally true of an unasserted mode.
            assert!(matches!(
                m.support(mode).reason(),
                Some(UnsupportedReason::NoNativeMode {
                    requested: None,
                    proven,
                }) if proven.is_empty()
            ));
        }
        let a = ActionMatrix::default();
        assert_eq!(a.support(ActionKind::Start), ActionSupport::Unsupported);
        assert_eq!(a.support(ActionKind::SetLevel), ActionSupport::Unsupported);
        let d = CapabilityDescriptor::claim_nothing(BackendId::new("x"));
        assert!(d.inconsistencies().is_empty());
        assert_eq!(d.clock, ClockDomain::Unknown);
        assert_eq!(d.abort_latency_bound, None);
    }

    /// Reconstruct the OLD UI's hand-written fiction - a device-owned-loop backend
    /// (a delegated controller) advertised as running Sine/Random/Shock/SRS - and prove the type
    /// system's validator rejects it. The honest claim (delegated OpenLoopSine only,
    /// matching the real backend's `native_modes`) passes.
    #[test]
    fn the_old_hand_written_fiction_is_flagged() {
        let mut lie = CapabilityDescriptor::claim_nothing(BackendId::new("delegated-device"));
        lie.loop_owner = LoopOwner::DeviceClosesLoop;
        // The fiction: claiming the closed-loop suite runs on this backend. On a
        // device-owned loop the host cannot synthesize the drive, so a
        // HostSynthesized claim here is structurally impossible.
        lie.modes.sine = ModeSupport::hosted();
        lie.modes.random = ModeSupport::hosted();
        lie.modes.shock = ModeSupport::hosted();
        lie.modes.srs = ModeSupport::hosted();
        let found = lie.inconsistencies();
        assert!(
            found.contains(&CapsInconsistency::HostedModeOnDeviceOwnedLoop {
                mode: TestMode::Random
            }),
            "{found:?}"
        );
        // Every one of the four fictional claims is individually flagged.
        assert_eq!(
            found
                .iter()
                .filter(|i| matches!(i, CapsInconsistency::HostedModeOnDeviceOwnedLoop { .. }))
                .count(),
            4
        );

        // The honest delegated-device descriptor: ONE delegated native mode, everything else
        // unsupported (per the real backend's caps).
        let mut honest = CapabilityDescriptor::claim_nothing(BackendId::new("delegated-device"));
        honest.loop_owner = LoopOwner::DeviceClosesLoop;
        honest.modes.open_loop_sine = ModeSupport::delegated(TestMode::OpenLoopSine);
        assert!(honest.inconsistencies().is_empty());
        assert!(!honest.modes.support(TestMode::Random).is_supported());
        assert!(honest.modes.support(TestMode::OpenLoopSine).is_delegated());
    }

    #[test]
    fn hosted_closed_loop_requires_a_proven_shared_clock() {
        // Mirrors the HAL rule: per-tick host adaptation refuses until the clock is
        // PROVEN Shared.
        let mut d = CapabilityDescriptor::claim_nothing(BackendId::new("ni"));
        d.loop_owner = LoopOwner::HostClosesLoop;
        d.modes.random = ModeSupport::hosted(); // per-tick correction
                                                // Host-synthesized OPEN loop: no correction, so no clock demand.
        d.modes.shock = ModeSupport::Supported(Resolution::new(
            DriveOrigin::HostSynthesized,
            AdaptationOwner::None,
        ));
        assert_eq!(
            d.inconsistencies(),
            vec![CapsInconsistency::HostedClosedLoopWithoutSharedClock {
                mode: TestMode::Random
            }]
        );
        d.clock = ClockDomain::Shared;
        assert!(d.inconsistencies().is_empty());
    }

    #[test]
    fn delegated_claims_need_a_device_owned_loop() {
        let mut d = CapabilityDescriptor::claim_nothing(BackendId::new("sim"));
        d.loop_owner = LoopOwner::HostClosesLoop;
        d.modes.open_loop_sine = ModeSupport::delegated(TestMode::OpenLoopSine);
        assert_eq!(
            d.inconsistencies(),
            vec![CapsInconsistency::DelegatedModeOnHostOwnedLoop {
                mode: TestMode::OpenLoopSine
            }]
        );
    }

    // =================================================================================
    // v2: the two-axis shape (wire bump #1)
    // =================================================================================

    /// The cell the v1 single-axis enum could NOT name: the device replays a frozen
    /// record and the HOST iterates between plays (TWR on a smart controller).
    /// Calling it "Delegated" told the UI the device was closing a loop it is not
    /// closing; calling it "Hosted" claimed a host drive path that does not exist.
    ///
    /// Red-mutation: collapse `ModeSupport` back to three valueless variants and this
    /// test cannot even be written - which is the point of the bump.
    #[test]
    fn the_converging_delegated_cell_is_expressible_and_consistent() {
        let mut d = CapabilityDescriptor::claim_nothing(BackendId::new("delegated-device"));
        d.loop_owner = LoopOwner::DeviceClosesLoop;
        d.modes.random = ModeSupport::Supported(Resolution::new(
            DriveOrigin::DeviceReplay {
                mode: TestMode::Random,
            },
            AdaptationOwner::HostBetweenRuns,
        ));
        assert!(
            d.inconsistencies().is_empty(),
            "device plays + host iterates between plays is a REAL cell: {:?}",
            d.inconsistencies()
        );
        let res = d.modes.support(TestMode::Random).resolution().unwrap();
        assert!(res.is_delegated_drive());
        assert!(!res.is_host_per_tick(), "between-runs is not per-tick");
        // ... and it does NOT demand a Shared clock, unlike per-tick correction.
        assert_eq!(d.clock, ClockDomain::Unknown);
    }

    /// The other v1-inexpressible cell: frozen device playback with NOTHING
    /// correcting. Distinct from full delegation, and the UI must not draw
    /// adaptation affordances for it.
    #[test]
    fn frozen_device_playback_is_distinct_from_full_delegation() {
        let playback = ModeSupport::Supported(Resolution::new(
            DriveOrigin::DeviceReplay {
                mode: TestMode::Random,
            },
            AdaptationOwner::None,
        ));
        let full = ModeSupport::delegated(TestMode::Random);
        assert_ne!(playback, full);
        // Both are "delegated" by the v1 question - which is exactly why one axis was
        // not enough.
        assert!(playback.is_delegated() && full.is_delegated());
        assert_eq!(
            playback.resolution().unwrap().adaptation,
            AdaptationOwner::None
        );
        assert_eq!(
            full.resolution().unwrap().adaptation,
            AdaptationOwner::Device
        );
    }

    /// The Recorder's shape: no drive at all, nothing correcting. Falls out of the
    /// model with no special case, and stays consistent.
    #[test]
    fn the_capture_only_shape_needs_no_special_case() {
        let mut d = CapabilityDescriptor::claim_nothing(BackendId::new("ni"));
        d.loop_owner = LoopOwner::HostClosesLoop;
        d.modes.open_loop_sine =
            ModeSupport::Supported(Resolution::new(DriveOrigin::None, AdaptationOwner::None));
        assert!(d.inconsistencies().is_empty());
        assert!(!d
            .modes
            .support(TestMode::OpenLoopSine)
            .resolution()
            .unwrap()
            .has_drive());
    }

    /// A correction owner with no drive to correct is incoherent - and the two-axis
    /// shape is the first version that can even STATE it, so it is the first that has
    /// to catch it.
    #[test]
    fn adaptation_without_a_drive_path_is_flagged() {
        let mut d = CapabilityDescriptor::claim_nothing(BackendId::new("ni"));
        d.loop_owner = LoopOwner::HostClosesLoop;
        d.clock = ClockDomain::Shared;
        d.modes.random = ModeSupport::Supported(Resolution::new(
            DriveOrigin::None,
            AdaptationOwner::HostPerTick,
        ));
        assert!(d
            .inconsistencies()
            .contains(&CapsInconsistency::AdaptationWithoutDrive {
                mode: TestMode::Random
            }));
    }

    /// A refusal carries the TYPED reason the operator has to act on, and it survives
    /// the wire round-trip. A bare "unsupported" cannot tell an operator whether to
    /// plug in a clock route or regenerate a profile.
    #[test]
    fn a_refusal_names_the_precondition_that_failed() {
        let clock = ModeSupport::Unsupported(UnsupportedReason::ClockNotShared {
            clock: ClockDomain::Unknown,
        });
        let rate = ModeSupport::Unsupported(UnsupportedReason::RateMismatch {
            profile_rate_hz: 51_200.0,
            actual_rate_hz: 51_200.0 * 1.005,
            relative_error: 0.005,
            regeneration_offered: true,
        });
        let no_mode = ModeSupport::Unsupported(UnsupportedReason::NoNativeMode {
            requested: Some(TestMode::Random),
            proven: vec![TestMode::OpenLoopSine],
        });
        for m in [&clock, &rate, &no_mode] {
            let text = serde_json::to_string(m).unwrap();
            let back: ModeSupport = serde_json::from_str(&text).unwrap();
            assert_eq!(&back, m, "{text}");
            assert!(!back.is_supported());
        }
        // The three are DISTINGUISHABLE - which is the whole reason the reason is typed.
        assert_ne!(clock, rate);
        assert_ne!(rate, no_mode);
        assert!(matches!(
            no_mode.reason(),
            Some(UnsupportedReason::NoNativeMode {
                requested: Some(TestMode::Random),
                ..
            })
        ));
    }

    /// Every supported shape round-trips through JSON with its resolution intact -
    /// the wire is where this type earns its keep.
    #[test]
    fn every_resolution_shape_round_trips_on_the_wire() {
        let shapes = [
            ModeSupport::hosted(),
            ModeSupport::delegated(TestMode::Random),
            ModeSupport::Supported(Resolution::new(
                DriveOrigin::HostSynthesized,
                AdaptationOwner::HostBetweenRuns,
            )),
            ModeSupport::Supported(Resolution::new(
                DriveOrigin::HostSynthesized,
                AdaptationOwner::None,
            )),
            ModeSupport::Supported(Resolution::new(DriveOrigin::None, AdaptationOwner::None)),
            ModeSupport::Supported(Resolution::new(
                DriveOrigin::DeviceReplay {
                    mode: TestMode::Srs,
                },
                AdaptationOwner::HostBetweenRuns,
            )),
        ];
        for m in shapes {
            let text = serde_json::to_string(&m).unwrap();
            let back: ModeSupport = serde_json::from_str(&text).unwrap();
            assert_eq!(back, m, "{text}");
        }
    }

    /// The station classes cross the seam because the UI badges them differently, and
    /// the abort-latency rule binds hardware stations only.
    #[test]
    fn station_classes_mirror_the_abi_hardware_predicate() {
        assert!(StationClass::NiSimulated.is_hardware());
        assert!(StationClass::NiReal.is_hardware());
        assert!(
            !StationClass::EigenSim.is_hardware(),
            "the physics plant has nothing to abort"
        );
        let text = serde_json::to_string(&StationClass::NiSimulated).unwrap();
        assert_eq!(text, "\"ni_simulated\"");
    }
}
