//! `eigen-module-abi` - Layer 4 module ABI.
//!
//! This is the crash + version boundary. Plain `#[repr(C)]` data plus an `extern "C"`
//! vtable, mirrored by the committed C-header floor at `abi/eigen_module_abi.h` (CI
//! diffs the generated layout against it). There is no licensing boundary: every module
//! is MIT and fully enabled. The per-module entitlement (granted feature codes, the
//! `EntitlementDenied` status) was removed in 0.3.
//!
//! Types pinned by the integrator. The `#[repr(C)]` boundary types are plain data
//! (no rich Rust types in any C-visible layout); the host translates rich types to and
//! from these views. HOST-SIDE logic that needs both the module's `LoopKind` and the
//! backend's `Caps` also lives here (see [`resolve_loop`]): this crate already sees
//! `eigen-hal`, while the engine (L2) must not depend on this crate (L4) and the HAL
//! (L3) must not depend on `LoopKind`.
#![allow(dead_code)]

use core::ffi::{c_char, c_void};
use eigen_hal::{Caps, ClockDomain, LoopOwner, NativeMode};
use std::fmt;

/// ABI version. A non-additive change bumps `major`; additive bumps `minor`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbiVersion {
    pub major: u32,
    pub minor: u32,
}

/// 0.2: added `LoopKind::Delegated = 2` (additive; the C floor gained
/// `EIGEN_LOOP_DELEGATED = 2`). The ABI remains UNFROZEN (major 0) - the freeze is
/// the integrator's call, not implied by this bump.
///
/// 0.3: removed the module licensing surface (`Entitlement`, the manifest's feature
/// codes, `configure`'s entitlement argument and `AbiStatus::EntitlementDenied`).
/// Non-additive, so by the rule above it would bump `major`; it stays a minor bump
/// because the ABI is unfrozen and no module has ever been built against 0.2. Status
/// value 2 is retired, not reused.
pub const ABI_VERSION: AbiVersion = AbiVersion { major: 0, minor: 3 };

/// Status codes returned across the boundary. `#[repr(i32)]` for a stable C enum.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiStatus {
    Ok = 0,
    Error = 1,
    // 2 was `EntitlementDenied` until 0.3. Retired: never reuse it.
    DimMismatch = 3,
    NotConfigured = 4,
    ClockRefused = 5,
}

/// Informational loop expectation the module declares. The real loop owner is the
/// (module, backend) pair resolved by the host - see [`resolve_loop`].
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopKind {
    OpenLoop = 0,
    ClosedLoop = 1,
    /// The module drives a smart controller's NATIVE mode (config download + start +
    /// sparse level commands) while the DEVICE generates every drive sample; the host
    /// only observes. It never fills a per-tick `drive_ptr` - see
    /// the delegated backend's ABI note for why it does not fit `TickCtx`, and
    /// note the delegated VTABLE shape is deliberately NOT designed yet (the vtable
    /// freeze is the integrator's call). This arm exists so the loop resolver can
    /// route a delegated module to a `DeviceClosesLoop` backend and NEVER attempt
    /// host drive math for it.
    Delegated = 2,
}

// ===========================================================================
// Loop resolution: TWO AXES, not one enum
// (a design change accepted 2026-07-16; it replaces the earlier
// three-valued form.)
// ===========================================================================

/// Axis 1: WHO PRODUCES THE DRIVE SAMPLES?
///
/// HOST-SIDE ONLY (deliberately not `#[repr(C)]`, not part of the C floor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveOrigin {
    /// No drive path at all. The Recorder's shape: capture only.
    None,
    /// Eigen computes and streams every sample through `write_drive`.
    HostSynthesized,
    /// Eigen downloads a reference/config and the DEVICE plays it from its own
    /// memory. Carries WHICH native mode, so the arm path configures exactly
    /// what was resolved.
    DeviceReplay(NativeMode),
}

/// Axis 2: WHO COMPUTES CORRECTIONS, AND ON WHAT TIMESCALE?
///
/// The axis the three-valued enum was missing. For CONTINUOUS modules the two
/// axes coincide (whoever synthesizes the samples is necessarily whoever corrects
/// them, because the correction IS the next sample), which is why the conflation
/// went unnoticed - the original design was written from Sine and Random. For
/// TRANSIENT / ITERATIVE modules they separate: the loop closes on a timescale of
/// seconds (play -> measure -> correct -> play), so "who plays" and "who
/// corrects" are genuinely independent. TWR is the case that proves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// One resolved (or requested) loop assignment: both axes together.
///
/// This is BOTH the request shape and the answer shape, deliberately. A module
/// asks for the loop it wants; the resolver either confirms exactly that
/// (`Resolved::Supported`) or refuses with a typed reason. It never returns a
/// DIFFERENT, weaker resolution than the one asked for - see [`resolve_loop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution {
    pub drive: DriveOrigin,
    pub adaptation: AdaptationOwner,
}

impl Resolution {
    /// The common constructor.
    pub const fn new(drive: DriveOrigin, adaptation: AdaptationOwner) -> Self {
        Resolution { drive, adaptation }
    }

    /// `{HostSynthesized, HostPerTick}` - the continuous host-loop shape
    /// (Sine servo, Random COLA). Requires a `Shared` clock.
    pub const fn host_closed_loop() -> Self {
        Resolution::new(DriveOrigin::HostSynthesized, AdaptationOwner::HostPerTick)
    }

    /// `{HostSynthesized, HostBetweenRuns}` - the iterative host-loop shape
    /// (TWR, SRS, host-compensated Shock). Does NOT require a `Shared` clock.
    pub const fn host_iterative() -> Self {
        Resolution::new(
            DriveOrigin::HostSynthesized,
            AdaptationOwner::HostBetweenRuns,
        )
    }

    /// `{HostSynthesized, None}` - host open-loop synthesis (SMART on NI).
    pub const fn host_open_loop() -> Self {
        Resolution::new(DriveOrigin::HostSynthesized, AdaptationOwner::None)
    }

    /// `{None, None}` - capture only (Recorder). Falls out of the model with no
    /// special case.
    pub const fn capture_only() -> Self {
        Resolution::new(DriveOrigin::None, AdaptationOwner::None)
    }

    /// `{DeviceReplay(m), Device}` - full delegation: the device plays AND
    /// corrects.
    pub const fn delegated(mode: NativeMode) -> Self {
        Resolution::new(DriveOrigin::DeviceReplay(mode), AdaptationOwner::Device)
    }

    /// `{DeviceReplay(m), None}` - the device replays a frozen record and
    /// NOTHING corrects. TWR scheduled playback, and the cell the three-valued
    /// enum could not name (see the type-level note on [`Resolved`]).
    pub const fn delegated_playback(mode: NativeMode) -> Self {
        Resolution::new(DriveOrigin::DeviceReplay(mode), AdaptationOwner::None)
    }

    /// `{DeviceReplay(m), HostBetweenRuns}` - the CONVERGING delegated cell:
    /// the device replays, the host iterates between plays. Requires
    /// `caps.delegated_response_observable`.
    pub const fn delegated_converging(mode: NativeMode) -> Self {
        Resolution::new(
            DriveOrigin::DeviceReplay(mode),
            AdaptationOwner::HostBetweenRuns,
        )
    }

    /// Is there any drive path at all? (`false` only for the Recorder shape.)
    pub fn has_drive(&self) -> bool {
        !matches!(self.drive, DriveOrigin::None)
    }
}

/// Why a requested loop assignment cannot be honored.
///
/// TYPED, not a string: the operator needs to know WHICH precondition failed in
/// order to act (plug in a chassis clock route vs. wait for a telemetry gap to
/// close vs. regenerate a profile), and a UI cannot branch on prose. Every
/// variant's [`fmt::Display`] is written to be read verbatim on the front
/// panel: it says what was asked, what the device actually proves, and what the
/// operator can do about it.
///
/// `PartialEq` but NOT `Eq`: [`UnsupportedReason::RateMismatch`] carries `f64`
/// rates, and float equality is not an equivalence relation. Tests compare
/// variants with `assert_eq!` on exact values they themselves constructed,
/// which is the one case where float equality is meaningful.
#[derive(Debug, Clone, PartialEq)]
pub enum UnsupportedReason {
    /// A device replay was requested for a mode the device does not PROVE
    /// (`requested: Some`), or delegation was requested with no mode named at
    /// all (`requested: None`).
    ///
    /// `caps.native_modes` is an evidence claim, not an inventory: a delegate
    /// path that is fully BUILT in software stays refused until the device's
    /// caps list the mode. This refusal is the gate that flips when HARDWARE
    /// PROOF lands, not when code lands.
    NoNativeMode {
        requested: Option<NativeMode>,
        proven: Vec<NativeMode>,
    },
    /// Per-tick host adaptation was requested against a clock domain that is
    /// not proven `Shared`. The clock rule, now an instance of the general
    /// precondition rule rather than a special case.
    ClockNotShared { clock: ClockDomain },
    /// Host adaptation BETWEEN runs was requested over a device replay, but the
    /// measured response does not come back to the host, so the iteration has
    /// no error signal.
    ///
    /// The resolver REFUSES rather than silently degrading to
    /// `{DeviceReplay, None}`: open-loop replay is a legitimate mode the
    /// operator reaches by ASKING for playback, never by the resolver quietly
    /// running a weaker test than the one requested.
    ResponseNotObservable { mode: NativeMode },
    /// A drive path was requested on a HARDWARE station with no abort-latency
    /// bound - measured or provisional. Produced by [`arm_gate`], not by
    /// [`resolve_loop`]: it is a station-class rule, not a loop-shape rule.
    NoAbortLatencyBound { station: StationClass },
    /// Delegation was requested of a backend that runs no control loop of its
    /// own (a DAQ card, the Sim plant). There is nothing to delegate TO.
    DelegationTargetIsHostLoop { requested: Option<NativeMode> },
    /// Host-synthesized drive was requested of a backend that generates its own
    /// drive. There is no per-sample host output path to insert control into,
    /// by design rather than as a temporary block.
    NoHostDrivePath { proven: Vec<NativeMode> },
    /// Correction was requested for a request that has no drive path to
    /// correct. An incoherent pairing, named rather than silently dropped.
    NothingToAdapt { adaptation: AdaptationOwner },
    /// The profile was generated at one rate and the hardware is configured at
    /// another. Produced by [`arm_gate`]'s rate policy.
    ///
    /// **Eigen NEVER resamples to close this gap.** Resampling would change the
    /// spectrum of the test the operator approved, silently, at arm time.
    RateMismatch {
        profile_rate_hz: f64,
        actual_rate_hz: f64,
        /// `|profile - actual| / actual`.
        relative_error: f64,
        /// Within the "regenerate the profile" band (<= 1%) rather than the
        /// gross-mismatch band. Both refuse; the offer differs.
        regeneration_offered: bool,
    },
}

impl fmt::Display for UnsupportedReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnsupportedReason::NoNativeMode {
                requested: Some(mode),
                proven,
            } => write!(
                f,
                "the device closes its own control loop and does not prove native {mode:?} \
                 (proven native modes: {proven:?}). A delegate path that is built in software \
                 but unproven on this hardware stays refused until the device's caps list it - \
                 arm a proven mode, or run this test on a host-loop backend."
            ),
            UnsupportedReason::NoNativeMode {
                requested: None,
                proven,
            } => write!(
                f,
                "the device closes its own control loop, so Eigen's host control math cannot be \
                 inserted into it, and this module requests no native device mode to delegate to \
                 (proven native modes: {proven:?}). Run this test on a host-loop backend."
            ),
            UnsupportedReason::ClockNotShared { clock } => write!(
                f,
                "per-tick host correction needs the input and output on the SAME physical clock, \
                 and this configuration's clock domain is {clock:?}, not Shared. Without a shared \
                 clock the drive and the measurement drift apart within a run and the loop \
                 corrects against stale phase. Prove the shared clock on this exact hardware \
                 configuration (bench loopback plus its negative control), or run a test whose \
                 correction happens BETWEEN runs instead of inside them."
            ),
            UnsupportedReason::ResponseNotObservable { mode } => write!(
                f,
                "this test corrects the drive between plays, which needs the measured response \
                 back on the host - and while the device replays native {mode:?} the response is \
                 NOT observable here. Refused rather than quietly run open-loop: replaying a \
                 frozen drive without adapting is a legitimate, different test, and you get it by \
                 asking for playback. Run the converging test on a host-loop backend, or wait for \
                 the delegated response path."
            ),
            UnsupportedReason::NoAbortLatencyBound { station } => write!(
                f,
                "this run drives an output on a {station} station, and no abort-latency bound \
                 exists for the configuration - not a measured one and not a provisional one. \
                 Without a bound nothing can state how long a stop takes to reach the shaker, so \
                 the run is refused rather than armed against an unknown. Configure the backend \
                 so the bound can be derived, or use a station that publishes one."
            ),
            UnsupportedReason::DelegationTargetIsHostLoop { requested } => {
                let named = match requested {
                    Some(m) => format!(" ({m:?})"),
                    None => String::new(),
                };
                write!(
                    f,
                    "this module drives a smart controller's native mode{named}, but the selected \
                     backend is host-loop hardware (Eigen generates the drive); there is no device \
                     loop to delegate to. Select the smart-controller backend this module targets, \
                     or a host-loop module."
                )
            }
            UnsupportedReason::NoHostDrivePath { proven } => write!(
                f,
                "this test needs Eigen to stream drive samples, but the selected backend generates \
                 its own drive and exposes no per-sample host output path (proven native modes: \
                 {proven:?}). Run this test on a host-loop backend, or ask for one of the modes the \
                 device proves."
            ),
            UnsupportedReason::NothingToAdapt { adaptation } => write!(
                f,
                "{adaptation:?} correction was requested, but this run has no drive path to \
                 correct. A capture-only run cannot adapt anything."
            ),
            UnsupportedReason::RateMismatch {
                profile_rate_hz,
                actual_rate_hz,
                relative_error,
                regeneration_offered: true,
            } => write!(
                f,
                "the profile was generated at {profile_rate_hz} Hz but the hardware is configured \
                 at {actual_rate_hz} Hz ({:.4}% apart). Eigen does not resample a profile to close \
                 this gap - resampling would change the spectrum of the test you approved, \
                 silently, at arm time. Regenerate the profile at the actual rate \
                 ({actual_rate_hz} Hz) and arm again.",
                relative_error * 100.0
            ),
            UnsupportedReason::RateMismatch {
                profile_rate_hz,
                actual_rate_hz,
                relative_error,
                regeneration_offered: false,
            } => write!(
                f,
                "the profile was generated at {profile_rate_hz} Hz but the hardware is configured \
                 at {actual_rate_hz} Hz - {:.2}% apart, far outside anything a regeneration would \
                 paper over. This is a configuration error, not a rounding difference: something \
                 selected a rate the profile was never written for. Fix the acquisition rate or \
                 the profile, then arm again. Eigen never resamples.",
                relative_error * 100.0
            ),
        }
    }
}

impl std::error::Error for UnsupportedReason {}

/// The answer: exactly the requested resolution, or a typed refusal.
///
/// HOST-SIDE ONLY (deliberately not `#[repr(C)]`, not part of the C floor): the
/// resolution is computed by the host from the manifest + caps and never crosses
/// the module boundary.
///
/// ## What replaced what, and why
///
/// v1 collapsed this to `caps.loop_owner` for all modules, which silently made "Sine on
/// a delegated controller" and "Sine on NI" two different control systems behind one
/// seam. A three-valued `{HostClosesLoop, DelegateToNative(m), Unsupported}` replaced
/// that. A 2026-07-16 design change replaced THAT, because the three-valued form
/// conflates "who generates the drive samples" with "who closes the control loop" - a
/// conflation that is harmless for continuous modules and wrong for iterative
/// ones. Concretely, TWR on a delegated controller replaying a converged drive record without
/// adapting is `{DeviceReplay, None}`, and the old enum's only honest answers
/// were `DelegateToNative` (which would falsely imply the device owns a loop it
/// does not have) or `Unsupported` (which would refuse a mode that genuinely
/// works). The five crates that each invented their own local vocabulary for
/// the missing axis - `TwrPathCapabilities`, shock's `PathCapabilities`,
/// `FirePath`, `delegated_path: bool`, `ModeSupport` - are the evidence that
/// the gap was real and not TWR-specific.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    /// The request stands, exactly as asked.
    Supported(Resolution),
    /// `arm()` must HARD-REFUSE and show `reason` to the operator verbatim.
    Unsupported { reason: UnsupportedReason },
}

impl Resolved {
    /// The resolution, or `None` if refused.
    pub fn supported(self) -> Option<Resolution> {
        match self {
            Resolved::Supported(r) => Some(r),
            Resolved::Unsupported { .. } => None,
        }
    }
}

/// Which kind of station a run is arming against.
///
/// Not a backend property: `NiSimulated` and `NiReal` are the SAME backend
/// distinguished by what the DRIVER says about the devices, and `EigenSim` is a
/// different backend entirely. Rules that treat them differently say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StationClass {
    /// The `eigen-hal-sim` physics plant. Control-law truth lives here; there is
    /// no hardware, no armature, and nothing to abort.
    EigenSim,
    /// The NI backend on driver-simulated devices (`DevIsSimulated`, amber
    /// badge). Real driver, real task machinery, no physical output.
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

impl fmt::Display for StationClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StationClass::EigenSim => write!(f, "EIGEN-SIM"),
            StationClass::NiSimulated => write!(f, "NI-SIM (driver-simulated)"),
            StationClass::NiReal => write!(f, "NI-REAL"),
        }
    }
}

/// Rate agreement at or below this relative error is EXACT for arming purposes:
/// floating-point representation noise, nothing more.
pub const RATE_EXACT_TOLERANCE: f64 = 1e-6;

/// Relative error above `RATE_EXACT_TOLERANCE` and at or below this is a
/// REFUSAL that offers profile regeneration. Above it, the refusal is a plain
/// configuration error. Eigen never resamples in either band.
pub const RATE_REGENERATION_BAND: f64 = 0.01;

/// Resolve who owns each axis for one (module, backend) pair.
///
/// Takes the module's REQUEST as a [`Resolution`] and answers with exactly that
/// resolution or a typed refusal. **It never returns a different resolution than
/// the one requested** - the load-bearing implementation rule: a converging
/// request whose response path is missing is REFUSED
/// ([`UnsupportedReason::ResponseNotObservable`]), never silently degraded to
/// open-loop replay. On a delegated controller TWR is a playback today; the
/// operator reaches that cell by explicitly requesting playback.
///
/// ## Preconditions (in check order)
///
/// | Requested | Precondition | Refusal |
/// |---|---|---|
/// | `DeviceReplay(_)` on a host-loop backend | (none - incompatible) | `DelegationTargetIsHostLoop` |
/// | `HostSynthesized` on a device-loop backend | (none - incompatible) | `NoHostDrivePath` |
/// | `DeviceReplay(m)` | `m` in `caps.native_modes` | `NoNativeMode` |
/// | `Device` adaptation | a `DeviceReplay` drive on a device-loop backend | `DelegationTargetIsHostLoop` / `NoNativeMode` |
/// | `HostPerTick` | `caps.clock == Shared` AND a host drive path | `ClockNotShared` / `NoHostDrivePath` |
/// | `HostBetweenRuns` over `DeviceReplay` | `caps.delegated_response_observable` | `ResponseNotObservable` |
/// | any correction with no drive | (none - incoherent) | `NothingToAdapt` |
///
/// Two things worth stating because they are easy to get backwards:
///
/// * **`HostBetweenRuns` does NOT require a `Shared` clock.** There is no COLA
///   continuity to preserve across frames; each play is independent. Its
///   requirement is drive/response alignment WITHIN one play, which is weaker.
///   Consequence: TWR and SRS on NI are NOT gated behind the arrival-day
///   clock bench result, while Random and Sine are.
/// * **`caps.delegated_response_observable` is only consulted for a
///   `DeviceReplay` drive.** A host-loop backend already holds the response.
pub fn resolve_loop(caps: &Caps, request: Resolution) -> Resolved {
    let refuse = |reason| Resolved::Unsupported { reason };
    let device_backend = caps.loop_owner == LoopOwner::DeviceClosesLoop;

    // --- 1. Is the requested DRIVE ORIGIN even available on this backend? ----
    match request.drive {
        DriveOrigin::DeviceReplay(mode) => {
            if !device_backend {
                return refuse(UnsupportedReason::DelegationTargetIsHostLoop {
                    requested: Some(mode),
                });
            }
            // `native_modes` is an EVIDENCE CLAIM. Built-but-
            // unproven stays refused until hardware proof lands.
            if !caps.native_modes.contains(&mode) {
                return refuse(UnsupportedReason::NoNativeMode {
                    requested: Some(mode),
                    proven: caps.native_modes.clone(),
                });
            }
        }
        DriveOrigin::HostSynthesized => {
            if device_backend {
                return refuse(UnsupportedReason::NoHostDrivePath {
                    proven: caps.native_modes.clone(),
                });
            }
        }
        // Capture-only is available on anything that can read inputs.
        DriveOrigin::None => {}
    }

    // --- 2. Does the requested ADAPTATION have its precondition? ------------
    match request.adaptation {
        AdaptationOwner::None => {}
        AdaptationOwner::Device => match request.drive {
            // The device's own loop corrects the record it is playing. Its
            // existence is proven by the same `native_modes` evidence checked
            // above, so there is nothing further to verify here.
            DriveOrigin::DeviceReplay(_) => {}
            // Delegation of the CORRECTION with no device replay to correct.
            // On a host-loop backend there is no device loop at all; on a
            // device-loop backend the module simply named no mode.
            _ if !device_backend => {
                return refuse(UnsupportedReason::DelegationTargetIsHostLoop { requested: None });
            }
            _ => {
                return refuse(UnsupportedReason::NoNativeMode {
                    requested: None,
                    proven: caps.native_modes.clone(),
                });
            }
        },
        AdaptationOwner::HostPerTick => {
            match request.drive {
                DriveOrigin::HostSynthesized => {}
                DriveOrigin::None => {
                    return refuse(UnsupportedReason::NothingToAdapt {
                        adaptation: request.adaptation,
                    });
                }
                // Correcting INSIDE a run the device is replaying would need a
                // per-sample host path the replay model does not have.
                DriveOrigin::DeviceReplay(_) => {
                    return refuse(UnsupportedReason::NoHostDrivePath {
                        proven: caps.native_modes.clone(),
                    });
                }
            }
            // The clock rule, now an instance of the general rule.
            if caps.clock != ClockDomain::Shared {
                return refuse(UnsupportedReason::ClockNotShared { clock: caps.clock });
            }
        }
        AdaptationOwner::HostBetweenRuns => match request.drive {
            // No clock requirement: each play is independent, so there is no
            // COLA continuity to preserve across frames.
            DriveOrigin::HostSynthesized => {}
            // TWR's feedback-path precondition. The same KIND of thing as the
            // clock rule: without it `play()` returns no response and the
            // iteration has no error signal.
            DriveOrigin::DeviceReplay(mode) => {
                if !caps.delegated_response_observable {
                    return refuse(UnsupportedReason::ResponseNotObservable { mode });
                }
            }
            DriveOrigin::None => {
                return refuse(UnsupportedReason::NothingToAdapt {
                    adaptation: request.adaptation,
                });
            }
        },
    }

    Resolved::Supported(request)
}

/// The MIGRATION mapping: the old three-valued call's inputs, as a two-axis
/// request.
///
/// The three old values are a lossy projection of the two axes and map
/// mechanically. On the OUTPUT side it reads:
///
/// ```text
/// HostClosesLoop        -> Supported { HostSynthesized, HostPerTick }
/// DelegateToNative(m)   -> Supported { DeviceReplay(m), Device }
/// Unsupported           -> Unsupported { reason }
/// ```
///
/// This function is the INPUT side of the same mapping, and it refines one cell
/// the old outputs could not distinguish: old `HostClosesLoop` covered BOTH the
/// closed-loop module (`{HostSynthesized, HostPerTick}`) and the open-loop
/// module riding the same host synthesis path (`{HostSynthesized, None}`).
/// Naming them separately is the refinement, and it is what makes SMART-on-NI
/// expressible.
///
/// ## One deliberate strengthening
///
/// The old resolver matched `caps.loop_owner` ONLY - it never read the clock.
/// A `ClosedLoop` module therefore resolved to `HostClosesLoop` against an
/// `Unknown` clock, which is precisely the clock refusal the general
/// precondition table now covers. Under the new resolver that request
/// refuses with [`UnsupportedReason::ClockNotShared`]. That is a BEHAVIOR
/// CHANGE and an intended one: the old permission was a gap, not a contract.
pub fn legacy_request(module_kind: LoopKind, requested_native: Option<NativeMode>) -> Resolution {
    match (module_kind, requested_native) {
        (LoopKind::ClosedLoop, _) => Resolution::host_closed_loop(),
        (LoopKind::OpenLoop, _) => Resolution::host_open_loop(),
        (LoopKind::Delegated, Some(m)) => Resolution::delegated(m),
        // Delegation with no mode named. Coherent as a REQUEST ("let the device
        // correct") and refused for a nameable reason, which is exactly what
        // the old resolver did with this input.
        (LoopKind::Delegated, None) => Resolution::new(DriveOrigin::None, AdaptationOwner::Device),
    }
}

/// THE ARM GATE. One function every module's arm path calls before it opens a
/// backend.
///
/// Three checks, in order, each of which REFUSES rather than degrading:
///
/// 1. **Loop resolution** ([`resolve_loop`]) - is the requested loop assignment
///    even available on this backend?
/// 2. **Abort-latency rule** - a run that DRIVES an output on
///    a hardware station needs an abort-latency bound to exist. A measured
///    bound and a paper-derived provisional `[H]` bound both satisfy it; the
///    absence of both does not. `EigenSim` is exempt: there is no armature and
///    nothing to abort.
/// 3. **Rate policy** - the profile's rate against the rate the
///    hardware actually configured. `<= 1e-6` relative error accepts;
///    `<= 1%` refuses and offers regeneration; `> 1%` refuses harder.
///    **Never resamples**, in any band.
///
/// `profile_rate_hz` is `None` for a run with no rate-bearing profile (the
/// Recorder, an interactive sweep); the rate check is then skipped, because
/// there is no claim to contradict.
///
/// The typed reason travels back to the caller so a service can put it in an
/// error string verbatim today and a UI panel can branch on the variant later.
pub fn arm_gate(
    caps: &Caps,
    station: StationClass,
    request: Resolution,
    profile_rate_hz: Option<f64>,
    actual_rate_hz: f64,
) -> Result<Resolution, UnsupportedReason> {
    // 1. The loop itself.
    let resolution = match resolve_loop(caps, request) {
        Resolved::Supported(r) => r,
        Resolved::Unsupported { reason } => return Err(reason),
    };

    // 2. Abort latency. Binds every HARDWARE station that drives an output.
    //
    // The rule is deliberately caps-level rather than "only when limits are
    // armed": a drive path plus a shaker is the hazard, and a run with no
    // limits armed is not safer, it is less instrumented. `EigenSim` is exempt
    // because there is no physical output for an abort to race.
    if station.is_hardware() && resolution.has_drive() && caps.abort_latency_bound.is_none() {
        return Err(UnsupportedReason::NoAbortLatencyBound { station });
    }

    // 3. Rate policy. Compared against the ACTUAL configured rate, never the
    // requested one - the request is what someone asked for, the actual is what
    // the DSP and the profile will really see.
    if let Some(profile) = profile_rate_hz {
        if let Some(reason) = rate_policy(profile, actual_rate_hz) {
            return Err(reason);
        }
    }

    Ok(resolution)
}

/// The rate policy in isolation. `None` = accept.
///
/// Exposed so a caller can pre-check a profile against a configured rate
/// without arming - and so the three bands have one implementation rather than
/// one per module.
pub fn rate_policy(profile_rate_hz: f64, actual_rate_hz: f64) -> Option<UnsupportedReason> {
    // A non-positive or non-finite actual rate cannot be compared against, and
    // treating that as "no mismatch" would let the check pass vacuously - the
    // green-that-cannot-fail shape. Refuse.
    if !(actual_rate_hz.is_finite() && actual_rate_hz > 0.0 && profile_rate_hz.is_finite()) {
        return Some(UnsupportedReason::RateMismatch {
            profile_rate_hz,
            actual_rate_hz,
            relative_error: f64::INFINITY,
            regeneration_offered: false,
        });
    }
    let relative_error = (profile_rate_hz - actual_rate_hz).abs() / actual_rate_hz;
    if relative_error <= RATE_EXACT_TOLERANCE {
        return None;
    }
    Some(UnsupportedReason::RateMismatch {
        profile_rate_hz,
        actual_rate_hz,
        relative_error,
        regeneration_offered: relative_error <= RATE_REGENERATION_BAND,
    })
}

/// The per-tick view handed to a module. Channel-interleaved pointers with explicit
/// lengths; the module never owns the memory and must not retain the pointers.
#[repr(C)]
pub struct TickCtx {
    pub n_control: u32,
    pub n_monitor: u32,
    pub n_drive: u32,
    pub n_reference: u32,
    pub n_frames: u32,
    pub sample_rate_hz: f64,
    /// Input samples, channel-interleaved f32,
    /// length = (n_control + n_monitor + n_reference) * n_frames.
    pub input_ptr: *const f32,
    pub input_len: usize,
    /// Drive output the module fills, length = n_drive * n_frames.
    pub drive_ptr: *mut f32,
    pub drive_len: usize,
    /// Opaque reference-spec pointer (MIMO reference matrix / weighting), or null.
    pub reference_spec: *const c_void,
}

/// A module's published manifest.
#[repr(C)]
pub struct ModuleManifest {
    pub abi_version: AbiVersion,
    pub name: *const c_char,
    pub loop_kind: LoopKind,
}

/// Opaque module instance handle.
pub type ModuleHandle = *mut c_void;

/// The vtable a module exports. Every call is `extern "C"`; the module MUST seal panics
/// at each edge so a panic never unwinds across the boundary.
#[repr(C)]
pub struct ModuleVTable {
    pub create: extern "C" fn() -> ModuleHandle,
    pub destroy: extern "C" fn(ModuleHandle),
    pub configure: extern "C" fn(ModuleHandle) -> AbiStatus,
    pub arm: extern "C" fn(ModuleHandle) -> AbiStatus,
    pub tick: extern "C" fn(ModuleHandle, *mut TickCtx) -> AbiStatus,
    pub stop: extern "C" fn(ModuleHandle) -> AbiStatus,
}

/// What a module's entry point returns: manifest + vtable.
#[repr(C)]
pub struct ModuleDescriptor {
    pub manifest: ModuleManifest,
    pub vtable: ModuleVTable,
}

/// Every module cdylib exports `eigen_module_entry` with this signature.
pub type ModuleEntryFn = extern "C" fn() -> *const ModuleDescriptor;

/// The symbol name the host resolves after `dlopen`.
pub const MODULE_ENTRY_SYMBOL: &[u8] = b"eigen_module_entry\0";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_version_is_pinned() {
        assert_eq!(ABI_VERSION, AbiVersion { major: 0, minor: 3 });
    }

    #[test]
    fn status_discriminants_are_stable() {
        // These values are the C contract; changing them is a non-additive ABI break.
        // 2 is retired (the 0.3 licensing removal) and must not be reused.
        assert_eq!(AbiStatus::Ok as i32, 0);
        assert_eq!(AbiStatus::Error as i32, 1);
        assert_eq!(AbiStatus::DimMismatch as i32, 3);
        assert_eq!(AbiStatus::NotConfigured as i32, 4);
        assert_eq!(AbiStatus::ClockRefused as i32, 5);
    }

    #[test]
    fn loop_kind_discriminants_are_stable() {
        // The C contract (abi/eigen_module_abi.h EigenLoopKind); Delegated was the
        // 0.2 additive extension and must never renumber the first two.
        assert_eq!(LoopKind::OpenLoop as i32, 0);
        assert_eq!(LoopKind::ClosedLoop as i32, 1);
        assert_eq!(LoopKind::Delegated as i32, 2);
    }

    // =====================================================================
    // resolve_loop / arm_gate - the two-axis model
    // =====================================================================
    //
    // All caps here are SYNTHETIC test doubles. Deliberately: the resolver's
    // tests must not lean on (or tempt anyone to widen) a real backend's
    // `native_modes` declaration - `native_modes` is proof-of-hardware-evidence,
    // not a test fixture.

    /// A host-loop backend shape (Sim/NI-like): Eigen generates the drive.
    /// `Shared` clock, so the per-tick precondition is satisfiable.
    fn host_caps() -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: 4,
            max_output_channels: 1,
            abort_latency_bound: None,
            output_buffer: None,
            sample_rate_hz: 51_200.0,
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }

    /// A host-loop backend whose clock is NOT proven shared.
    fn host_caps_unknown_clock() -> Caps {
        Caps {
            clock: ClockDomain::Unknown,
            ..host_caps()
        }
    }

    /// A smart-controller shape (a delegated controller) proving exactly the given modes.
    /// `delegated_response_observable: false` mirrors the delegated declaration shipped today
    /// (the open delegated telemetry gap).
    fn device_caps_proving(native: &[NativeMode]) -> Caps {
        Caps {
            loop_owner: LoopOwner::DeviceClosesLoop,
            native_modes: native.to_vec(),
            clock: ClockDomain::Unknown,
            max_input_channels: 8,
            max_output_channels: 0,
            abort_latency_bound: None,
            output_buffer: None,
            sample_rate_hz: 65_536.0,
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }

    /// A hypothetical smart controller whose delegated response DOES come back.
    /// Exists only so the `HostBetweenRuns + DeviceReplay` precondition can be
    /// tested in BOTH directions - a refusal test whose precondition can never
    /// be satisfied cannot tell "refused correctly" from "always refuses".
    fn device_caps_with_observable_response(native: &[NativeMode]) -> Caps {
        Caps {
            delegated_response_observable: true,
            ..device_caps_proving(native)
        }
    }

    fn reason_of(r: Resolved) -> UnsupportedReason {
        match r {
            Resolved::Unsupported { reason } => reason,
            Resolved::Supported(res) => {
                panic!("expected a refusal, got Supported({res:?})")
            }
        }
    }

    // ---------------------------------------------------------------------
    // THE 14-CELL MATRIX (in table order)
    // ---------------------------------------------------------------------
    //
    // RED-MUTATION for the whole matrix: delete any ONE precondition check in
    // `resolve_loop` (the `native_modes.contains` test, the `clock == Shared`
    // test, the `delegated_response_observable` test, or either
    // backend/drive-origin compatibility test) and at least one cell below
    // flips from refusal to acceptance, reddening this test. It is the
    // executable form of the resolution table, so the table and the code
    // cannot drift apart silently.

    #[test]
    fn amendment_section_5_cell_table() {
        // The delegated controller declares exactly one proven native mode today.
        let smart_device = device_caps_proving(&[NativeMode::OpenLoopSine]);
        // A host-loop DAQ/Sim backend with a proven shared clock.
        let ni = host_caps();
        // ...and one whose clock is not proven, for the two cells that care.
        let ni_unproven = host_caps_unknown_clock();

        // --- 1. SMART / delegated controller: DeviceReplay(OpenLoopSine) + None -------------
        // "mode proven - the one live cell".
        assert_eq!(
            resolve_loop(
                &smart_device,
                Resolution::delegated_playback(NativeMode::OpenLoopSine)
            ),
            Resolved::Supported(Resolution::delegated_playback(NativeMode::OpenLoopSine)),
        );

        // --- 2. SMART / NI: HostSynthesized + None -------------------------
        // "expressible (a new host sweep synthesizer)". The cell the
        // three-valued enum could not distinguish from the closed-loop one.
        assert_eq!(
            resolve_loop(&ni, Resolution::host_open_loop()),
            Resolved::Supported(Resolution::host_open_loop()),
        );

        // --- 3. Random / delegated controller: DeviceReplay(Random) + Device ---------------
        // "fails: Random not in native_modes (never armed) -> Unsupported."
        assert_eq!(
            reason_of(resolve_loop(
                &smart_device,
                Resolution::delegated(NativeMode::Random)
            )),
            UnsupportedReason::NoNativeMode {
                requested: Some(NativeMode::Random),
                proven: vec![NativeMode::OpenLoopSine],
            },
        );

        // --- 4. Random / NI: HostSynthesized + HostPerTick -----------------
        // "needs clock == Shared". Both directions, because a refusal that
        // cannot succeed is not evidence that the gate reads the clock.
        assert_eq!(
            resolve_loop(&ni, Resolution::host_closed_loop()),
            Resolved::Supported(Resolution::host_closed_loop()),
        );
        assert_eq!(
            reason_of(resolve_loop(&ni_unproven, Resolution::host_closed_loop())),
            UnsupportedReason::ClockNotShared {
                clock: ClockDomain::Unknown
            },
        );

        // --- 5. Sine / delegated controller: DeviceReplay(Sine) + Device -------------------
        // "fails: not in native_modes, config image unpinned -> Unsupported."
        assert_eq!(
            reason_of(resolve_loop(
                &smart_device,
                Resolution::delegated(NativeMode::Sine)
            )),
            UnsupportedReason::NoNativeMode {
                requested: Some(NativeMode::Sine),
                proven: vec![NativeMode::OpenLoopSine],
            },
        );

        // --- 6. Sine / NI: HostSynthesized + HostPerTick -------------------
        // Same shape as Random/NI: the clock is the gate.
        assert_eq!(
            resolve_loop(&ni, Resolution::host_closed_loop()),
            Resolved::Supported(Resolution::host_closed_loop()),
        );

        // --- 7. Shock / delegated controller: DeviceReplay(Shock) + Device -----------------
        // "device owns the law... Currently Unsupported: Shock not in
        // native_modes." The host-side level ladder is a SEQUENCER, not a
        // corrector, which is why this cell is `Device` and not
        // `HostBetweenRuns`.
        assert_eq!(
            reason_of(resolve_loop(
                &smart_device,
                Resolution::delegated(NativeMode::Shock)
            )),
            UnsupportedReason::NoNativeMode {
                requested: Some(NativeMode::Shock),
                proven: vec![NativeMode::OpenLoopSine],
            },
        );

        // --- 8. Shock / NI: HostSynthesized + HostBetweenRuns --------------
        // "host compensation (Path B)". NOT gated on the clock.
        assert_eq!(
            resolve_loop(&ni, Resolution::host_iterative()),
            Resolved::Supported(Resolution::host_iterative()),
        );
        assert_eq!(
            resolve_loop(&ni_unproven, Resolution::host_iterative()),
            Resolved::Supported(Resolution::host_iterative()),
            "HostBetweenRuns has no COLA continuity to preserve, so an unproven \
             clock must NOT gate it - this is what keeps TWR/SRS on NI off the \
             arrival-day clock bench result"
        );

        // --- 9. SRS / delegated controller: DeviceReplay(Srs) + HostBetweenRuns ------------
        // "fails twice: no wire decode exists, and FirePath::Delegated has zero
        // implementors." The FIRST failure is the one the resolver can see:
        // Srs is not a proven native mode.
        assert_eq!(
            reason_of(resolve_loop(
                &smart_device,
                Resolution::delegated_converging(NativeMode::Srs)
            )),
            UnsupportedReason::NoNativeMode {
                requested: Some(NativeMode::Srs),
                proven: vec![NativeMode::OpenLoopSine],
            },
        );
        // ...and if the mode WERE proven, the response-path precondition is the
        // second failure, exactly as the table predicts. (Synthetic caps: this
        // is the "likely second hybrid", marked as a prediction.)
        let device_if_srs_proven = device_caps_proving(&[NativeMode::Srs]);
        assert_eq!(
            reason_of(resolve_loop(
                &device_if_srs_proven,
                Resolution::delegated_converging(NativeMode::Srs)
            )),
            UnsupportedReason::ResponseNotObservable {
                mode: NativeMode::Srs
            },
        );

        // --- 10. SRS / NI: HostSynthesized + HostBetweenRuns ---------------
        assert_eq!(
            resolve_loop(&ni, Resolution::host_iterative()),
            Resolved::Supported(Resolution::host_iterative()),
        );

        // --- 11. TWR / delegated controller (playback): DeviceReplay(..) + None ------------
        // "LEGAL - needs no response path. Blocked only by the mode-entry gap."
        // The mode-entry gap IS the `native_modes` refusal today...
        let twr_mode = NativeMode::Mimo; // stands for TWR's device-replay mode
        assert_eq!(
            reason_of(resolve_loop(
                &smart_device,
                Resolution::delegated_playback(twr_mode)
            )),
            UnsupportedReason::NoNativeMode {
                requested: Some(twr_mode),
                proven: vec![NativeMode::OpenLoopSine],
            },
        );
        // ...and once that gap closes, the cell is LEGAL even though the
        // response path is still missing. This is the capability the
        // three-valued enum would have hidden: `DelegateToNative` would falsely
        // imply a device loop, `Unsupported` would refuse a working mode.
        let device_if_twr_proven = device_caps_proving(&[twr_mode]);
        assert_eq!(
            resolve_loop(
                &device_if_twr_proven,
                Resolution::delegated_playback(twr_mode)
            ),
            Resolved::Supported(Resolution::delegated_playback(twr_mode)),
        );

        // --- 12. TWR / delegated controller (converging): DeviceReplay(..) + HostBetweenRuns
        // "fails: response_observable == false -> Unsupported, honestly and for
        // a nameable reason." REFUSED, not degraded to cell 11.
        assert_eq!(
            reason_of(resolve_loop(
                &device_if_twr_proven,
                Resolution::delegated_converging(twr_mode)
            )),
            UnsupportedReason::ResponseNotObservable { mode: twr_mode },
        );
        // And the precondition works in the OTHER direction too: give the same
        // controller an observable delegated response and the converging cell
        // resolves. Without this half, the refusal above could equally be
        // "converging delegation is never supported".
        let observable = device_caps_with_observable_response(&[twr_mode]);
        assert_eq!(
            resolve_loop(&observable, Resolution::delegated_converging(twr_mode)),
            Resolved::Supported(Resolution::delegated_converging(twr_mode)),
        );

        // --- 13. TWR / NI: HostSynthesized + HostBetweenRuns ---------------
        // "the cheapest NI port in the tree."
        assert_eq!(
            resolve_loop(&ni, Resolution::host_iterative()),
            Resolved::Supported(Resolution::host_iterative()),
        );

        // --- 14. Recorder / any: None + None -------------------------------
        // "CaptureOnly falls out; no special case needed." On BOTH backend
        // kinds, which is what "any" means.
        assert_eq!(
            resolve_loop(&ni, Resolution::capture_only()),
            Resolved::Supported(Resolution::capture_only()),
        );
        assert_eq!(
            resolve_loop(&smart_device, Resolution::capture_only()),
            Resolved::Supported(Resolution::capture_only()),
        );
        assert_eq!(
            resolve_loop(&ni_unproven, Resolution::capture_only()),
            Resolved::Supported(Resolution::capture_only()),
            "capture has no drive and no correction, so no clock precondition"
        );
    }

    // ---------------------------------------------------------------------
    // Preconditions, one at a time
    // ---------------------------------------------------------------------

    /// RED-MUTATION: delete the `caps.clock != ClockDomain::Shared` check in
    /// `resolve_loop`'s `HostPerTick` arm and the `Unknown`/`Independent` cases
    /// below red. This is the clock rule, and the OLD three-valued resolver
    /// never performed it at all.
    #[test]
    fn host_per_tick_refuses_an_unproven_clock_and_accepts_a_shared_one() {
        for clock in [ClockDomain::Unknown, ClockDomain::Independent] {
            let caps = Caps {
                clock,
                ..host_caps()
            };
            let reason = reason_of(resolve_loop(&caps, Resolution::host_closed_loop()));
            assert_eq!(reason, UnsupportedReason::ClockNotShared { clock });
            // The operator text must name the domain and say what to do.
            let text = reason.to_string();
            assert!(text.contains("Shared"), "{text}");
            assert!(text.contains("drift"), "{text}");
        }
        // The success half: a proven shared clock arms.
        assert_eq!(
            resolve_loop(&host_caps(), Resolution::host_closed_loop()),
            Resolved::Supported(Resolution::host_closed_loop()),
        );
    }

    /// RED-MUTATION: make the `HostBetweenRuns + DeviceReplay` arm fall through
    /// to `Supported` (or, worse, rewrite the request to
    /// `{DeviceReplay, None}`) and this test reds. Degrading instead of
    /// refusing would run a WEAKER test than the operator asked for, which is
    /// the specific behavior the resolver contract forbids.
    #[test]
    fn a_converging_request_is_refused_not_degraded_to_playback() {
        let caps = device_caps_proving(&[NativeMode::OpenLoopSine]);
        let converging = Resolution::delegated_converging(NativeMode::OpenLoopSine);
        let resolved = resolve_loop(&caps, converging);
        // NOT Supported({DeviceReplay, None}).
        assert!(
            !matches!(
                resolved,
                Resolved::Supported(Resolution {
                    adaptation: AdaptationOwner::None,
                    ..
                })
            ),
            "the resolver must never answer with a weaker resolution than the request"
        );
        assert_eq!(
            reason_of(resolved),
            UnsupportedReason::ResponseNotObservable {
                mode: NativeMode::OpenLoopSine
            },
        );
        // The same operator, asking for PLAYBACK, gets it.
        assert_eq!(
            resolve_loop(
                &caps,
                Resolution::delegated_playback(NativeMode::OpenLoopSine)
            ),
            Resolved::Supported(Resolution::delegated_playback(NativeMode::OpenLoopSine)),
        );
    }

    #[test]
    fn delegation_to_a_host_loop_backend_is_refused() {
        let caps = host_caps();
        assert_eq!(
            reason_of(resolve_loop(
                &caps,
                Resolution::delegated(NativeMode::OpenLoopSine)
            )),
            UnsupportedReason::DelegationTargetIsHostLoop {
                requested: Some(NativeMode::OpenLoopSine)
            },
        );
        let text = UnsupportedReason::DelegationTargetIsHostLoop {
            requested: Some(NativeMode::OpenLoopSine),
        }
        .to_string();
        assert!(text.contains("no device loop"), "{text}");
    }

    #[test]
    fn host_synthesized_drive_on_a_device_backend_is_refused() {
        let caps = device_caps_proving(&[NativeMode::OpenLoopSine]);
        for request in [
            Resolution::host_closed_loop(),
            Resolution::host_open_loop(),
            Resolution::host_iterative(),
        ] {
            assert_eq!(
                reason_of(resolve_loop(&caps, request)),
                UnsupportedReason::NoHostDrivePath {
                    proven: vec![NativeMode::OpenLoopSine],
                },
                "request {request:?}"
            );
        }
    }

    #[test]
    fn correction_with_no_drive_path_is_named_rather_than_ignored() {
        let caps = host_caps();
        for adaptation in [
            AdaptationOwner::HostPerTick,
            AdaptationOwner::HostBetweenRuns,
        ] {
            assert_eq!(
                reason_of(resolve_loop(
                    &caps,
                    Resolution::new(DriveOrigin::None, adaptation)
                )),
                UnsupportedReason::NothingToAdapt { adaptation },
            );
        }
    }

    // ---------------------------------------------------------------------
    // Migration: the old three values map mechanically
    // ---------------------------------------------------------------------

    /// RED-MUTATION: change any arm of `legacy_request` (e.g. map
    /// `LoopKind::ClosedLoop` to `{HostSynthesized, None}`) and this test reds.
    /// Section 6's mapping is normative; this is its executable form.
    #[test]
    fn migration_mapping_old_three_values_to_new_pairs() {
        // HostClosesLoop -> Supported { HostSynthesized, HostPerTick }
        //
        // Section 6 states the mapping on the RESOLVED value; a `ClosedLoop`
        // module against a host-loop backend is what produced old
        // `HostClosesLoop`, and it maps to the closed-loop pair.
        assert_eq!(
            legacy_request(LoopKind::ClosedLoop, None),
            Resolution::new(DriveOrigin::HostSynthesized, AdaptationOwner::HostPerTick),
        );
        assert_eq!(
            resolve_loop(&host_caps(), legacy_request(LoopKind::ClosedLoop, None)),
            Resolved::Supported(Resolution::host_closed_loop()),
        );

        // DelegateToNative(m) -> Supported { DeviceReplay(m), Device }
        let m = NativeMode::OpenLoopSine;
        assert_eq!(
            legacy_request(LoopKind::Delegated, Some(m)),
            Resolution::new(DriveOrigin::DeviceReplay(m), AdaptationOwner::Device),
        );
        assert_eq!(
            resolve_loop(
                &device_caps_proving(&[m]),
                legacy_request(LoopKind::Delegated, Some(m))
            ),
            Resolved::Supported(Resolution::delegated(m)),
        );

        // Unsupported -> Unsupported { reason } - and the reason is now TYPED,
        // which is the whole point of the refinement.
        assert_eq!(
            reason_of(resolve_loop(
                &device_caps_proving(&[NativeMode::OpenLoopSine]),
                legacy_request(LoopKind::Delegated, Some(NativeMode::Random))
            )),
            UnsupportedReason::NoNativeMode {
                requested: Some(NativeMode::Random),
                proven: vec![NativeMode::OpenLoopSine],
            },
        );
    }

    /// The REFINEMENT the old outputs could not express: old `HostClosesLoop`
    /// covered both the closed-loop module and the open-loop module riding the
    /// same host synthesis path. Naming them apart is what makes SMART-on-NI
    /// (`{HostSynthesized, None}`, section 5 row 2) expressible at all.
    ///
    /// RED-MUTATION: map `LoopKind::OpenLoop` to `host_closed_loop()` and this
    /// test reds - an open-loop sweep would then demand a proven shared clock
    /// it has no use for.
    #[test]
    fn migration_refines_the_ambiguous_host_closes_loop_cell() {
        assert_eq!(
            legacy_request(LoopKind::OpenLoop, None),
            Resolution::new(DriveOrigin::HostSynthesized, AdaptationOwner::None),
        );
        // ...and the practical consequence: open-loop host synthesis arms
        // against an unproven clock, where the closed-loop request would not.
        let unproven = host_caps_unknown_clock();
        assert_eq!(
            resolve_loop(&unproven, legacy_request(LoopKind::OpenLoop, None)),
            Resolved::Supported(Resolution::host_open_loop()),
        );
        assert!(matches!(
            resolve_loop(&unproven, legacy_request(LoopKind::ClosedLoop, None)),
            Resolved::Unsupported { .. }
        ));
    }

    #[test]
    fn migration_preserves_the_old_refusals_for_every_old_input_shape() {
        let host = host_caps();
        let device = device_caps_proving(&[NativeMode::OpenLoopSine]);

        // OLD: HostClosesLoop backend + Delegated module -> Unsupported
        // ("no device loop to delegate to"), with and without a named mode.
        assert_eq!(
            reason_of(resolve_loop(
                &host,
                legacy_request(LoopKind::Delegated, Some(NativeMode::OpenLoopSine))
            )),
            UnsupportedReason::DelegationTargetIsHostLoop {
                requested: Some(NativeMode::OpenLoopSine)
            },
        );
        assert_eq!(
            reason_of(resolve_loop(
                &host,
                legacy_request(LoopKind::Delegated, None)
            )),
            UnsupportedReason::DelegationTargetIsHostLoop { requested: None },
        );

        // OLD: DeviceClosesLoop backend + a module with no native request ->
        // Unsupported ("host control math cannot be inserted"). Both the
        // closed-loop and open-loop module shapes landed here.
        for kind in [LoopKind::ClosedLoop, LoopKind::OpenLoop] {
            assert_eq!(
                reason_of(resolve_loop(&device, legacy_request(kind, None))),
                UnsupportedReason::NoHostDrivePath {
                    proven: vec![NativeMode::OpenLoopSine],
                },
                "old input: {kind:?} module on a device-loop backend"
            );
        }

        // OLD: Delegated + no mode named, against a DEVICE backend -> the
        // "requests no native device mode to delegate to" refusal, preserved
        // verbatim in the `requested: None` Display arm. (Against a HOST
        // backend the same input lands on `DelegationTargetIsHostLoop`, which
        // is the assertion a few lines above.)
        let device_side = reason_of(resolve_loop(
            &device,
            legacy_request(LoopKind::Delegated, None),
        ));
        assert_eq!(
            device_side,
            UnsupportedReason::NoNativeMode {
                requested: None,
                proven: vec![NativeMode::OpenLoopSine],
            },
        );
        assert!(
            device_side.to_string().contains("no native device mode"),
            "{device_side}"
        );
    }

    // ---------------------------------------------------------------------
    // arm_gate: abort latency
    // ---------------------------------------------------------------------

    /// RED-MUTATION: delete the `caps.abort_latency_bound.is_none()` clause in
    /// `arm_gate` (or widen the exemption from `EigenSim` to every station) and
    /// the first two assertions red. This is the gate that stands between a
    /// drive path and a shaker when nothing can state how long a stop takes.
    #[test]
    fn a_hardware_station_with_a_drive_path_refuses_without_an_abort_bound() {
        let caps = host_caps(); // abort_latency_bound: None
        for station in [StationClass::NiSimulated, StationClass::NiReal] {
            let err = arm_gate(
                &caps,
                station,
                Resolution::host_closed_loop(),
                None,
                51_200.0,
            )
            .expect_err("no bound exists, so a driving run must refuse");
            assert_eq!(err, UnsupportedReason::NoAbortLatencyBound { station });
            let text = err.to_string();
            assert!(text.contains("abort-latency"), "{text}");
            assert!(text.contains("provisional"), "{text}");
        }

        // The SUCCESS half: a provisional bound satisfies the rule. Without
        // this, the refusal above could equally mean "hardware never arms",
        // and the bootstrap deadlock would be invisible.
        let bounded = Caps {
            abort_latency_bound: Some(std::time::Duration::from_millis(160)),
            ..host_caps()
        };
        for station in [StationClass::NiSimulated, StationClass::NiReal] {
            assert_eq!(
                arm_gate(
                    &bounded,
                    station,
                    Resolution::host_closed_loop(),
                    None,
                    51_200.0
                ),
                Ok(Resolution::host_closed_loop()),
            );
        }
    }

    /// RED-MUTATION: drop the `EigenSim` exemption (make `is_hardware()` return
    /// true for it) and this test reds - the whole zero-hardware development
    /// path would refuse to arm for want of a bound on an abort that has
    /// nothing physical to race.
    #[test]
    fn eigen_sim_is_exempt_from_the_abort_bound_rule() {
        let caps = host_caps(); // no bound
        assert_eq!(
            arm_gate(
                &caps,
                StationClass::EigenSim,
                Resolution::host_closed_loop(),
                None,
                51_200.0
            ),
            Ok(Resolution::host_closed_loop()),
        );
        assert!(!StationClass::EigenSim.is_hardware());
        assert!(StationClass::NiSimulated.is_hardware());
        assert!(StationClass::NiReal.is_hardware());
    }

    /// A capture-only run has no drive path, so the abort rule does not bind it
    /// even on real hardware: there is no output to keep driving.
    #[test]
    fn a_capture_only_run_needs_no_abort_bound() {
        let caps = host_caps();
        assert_eq!(
            arm_gate(
                &caps,
                StationClass::NiReal,
                Resolution::capture_only(),
                None,
                51_200.0
            ),
            Ok(Resolution::capture_only()),
        );
    }

    // ---------------------------------------------------------------------
    // arm_gate: the rate policy, all three bands
    // ---------------------------------------------------------------------

    /// RED-MUTATION: change `RATE_EXACT_TOLERANCE` to a loose value (say 1e-3),
    /// or make `rate_policy` return `None` in the regeneration band, and this
    /// test reds. The bands are exact numbers, not vibes, and the whole point
    /// is that Eigen refuses instead of resampling.
    #[test]
    fn rate_policy_has_exactly_three_bands_at_the_stated_thresholds() {
        let actual = 51_200.0_f64;

        // BAND 1 - accept. Exactly equal, and at the 1e-6 boundary.
        assert_eq!(rate_policy(actual, actual), None);
        let at_tolerance = actual * (1.0 + RATE_EXACT_TOLERANCE);
        assert_eq!(
            rate_policy(at_tolerance, actual),
            None,
            "1e-6 relative error is representation noise, not a mismatch"
        );

        // BAND 2 - refuse, and OFFER regeneration (just over 1e-6, up to 1%).
        let just_over = actual * (1.0 + 2.0 * RATE_EXACT_TOLERANCE);
        match rate_policy(just_over, actual).expect("must refuse") {
            UnsupportedReason::RateMismatch {
                regeneration_offered,
                profile_rate_hz,
                actual_rate_hz,
                ..
            } => {
                assert!(regeneration_offered);
                assert_eq!(profile_rate_hz, just_over);
                assert_eq!(actual_rate_hz, actual);
            }
            other => panic!("expected RateMismatch, got {other:?}"),
        }
        // At exactly 1%: still the regeneration band.
        let at_band_edge = actual * (1.0 + RATE_REGENERATION_BAND);
        let edge = rate_policy(at_band_edge, actual).expect("must refuse");
        assert!(matches!(
            edge,
            UnsupportedReason::RateMismatch {
                regeneration_offered: true,
                ..
            }
        ));
        // The offer must be READABLE - it names the actual rate to regenerate at.
        let text = edge.to_string();
        assert!(text.contains("Regenerate the profile"), "{text}");
        assert!(text.contains("51200"), "{text}");
        assert!(text.contains("does not resample"), "{text}");

        // BAND 3 - refuse, no offer, harsher text (> 1%).
        let gross = actual * 1.5;
        let harsh = rate_policy(gross, actual).expect("must refuse");
        assert!(matches!(
            harsh,
            UnsupportedReason::RateMismatch {
                regeneration_offered: false,
                ..
            }
        ));
        let text = harsh.to_string();
        assert!(text.contains("configuration error"), "{text}");
        assert!(text.contains("never resamples"), "{text}");

        // The bands are symmetric: a profile BELOW the actual rate is the same
        // three bands, not a special case.
        assert!(rate_policy(actual * 0.995, actual).is_some());
        assert!(rate_policy(actual * 0.5, actual).is_some());
    }

    /// RED-MUTATION: make `rate_policy` return `None` (accept) for a
    /// non-positive actual rate and this test reds. A comparison against zero
    /// would divide by zero and a NaN comparison silently accepts - the
    /// "green that cannot fail" shape.
    #[test]
    fn an_unusable_actual_rate_refuses_rather_than_passing_vacuously() {
        for actual in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(
                rate_policy(51_200.0, actual).is_some(),
                "actual rate {actual} must refuse, not pass"
            );
        }
        assert!(rate_policy(f64::NAN, 51_200.0).is_some());
    }

    #[test]
    fn a_run_with_no_profile_rate_skips_the_rate_check() {
        // The Recorder / an interactive sweep: no profile, so no claim to
        // contradict. The gate must not invent one.
        let caps = host_caps();
        assert_eq!(
            arm_gate(
                &caps,
                StationClass::EigenSim,
                Resolution::host_closed_loop(),
                None,
                51_200.0
            ),
            Ok(Resolution::host_closed_loop()),
        );
    }

    #[test]
    fn arm_gate_runs_the_resolver_first_so_a_loop_refusal_wins() {
        // A request that fails BOTH the loop resolution and the rate policy
        // must report the loop reason: fixing the profile rate would not make
        // the run possible, and the operator should be told the blocking fact.
        let caps = host_caps_unknown_clock();
        let err = arm_gate(
            &caps,
            StationClass::EigenSim,
            Resolution::host_closed_loop(),
            Some(1_000.0),
            51_200.0,
        )
        .expect_err("unproven clock");
        assert_eq!(
            err,
            UnsupportedReason::ClockNotShared {
                clock: ClockDomain::Unknown
            }
        );
    }

    #[test]
    fn arm_gate_returns_the_resolution_it_confirmed() {
        // Not a rewritten one: what comes back is what was asked for.
        let caps = host_caps();
        for request in [
            Resolution::host_closed_loop(),
            Resolution::host_open_loop(),
            Resolution::host_iterative(),
            Resolution::capture_only(),
        ] {
            assert_eq!(
                arm_gate(
                    &caps,
                    StationClass::EigenSim,
                    request,
                    Some(51_200.0),
                    51_200.0
                ),
                Ok(request),
            );
        }
    }
}
