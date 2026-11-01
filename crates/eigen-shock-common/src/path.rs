//! Path-honesty type seams.
//!
//! A shock test runs on one of two fundamentally different drive paths, and almost
//! every "advanced" feature (host compensation, shaped system ID, at-rate pulse
//! trains, drive V/I limiting, Eigen's own control law) is meaningful on ONLY ONE of
//! them. To keep the services and the UI from re-deriving "is this greyed?" in five
//! places (and drifting), the capability truth lives in ONE table, [`PathCapabilities`],
//! keyed off [`DrivePath`].
//!
//! # The two paths
//!
//! - [`DrivePath::Delegated`] (Path A): a delegated controller closes the
//!   loop on-device. Eigen downloads a reference waveform and fires absolute
//!   `10^(dB/20)` setpoints; the DEVICE owns compensation, system ID, and the control
//!   law. Eigen observes but does not compute the closed loop.
//! - [`DrivePath::HostDac`] (Path B): Eigen drives the DAC directly and closes the
//!   whole loop on the host. Compensation, synthesis, limiting, and the control law
//!   are all Eigen's to compute. This is the path the shock control math targets;
//!   fully host-driven SRS/limiting closes on Sim today (direct-DAC is still gated on
//!   the calibration decode).
//!
//! # The HARD invariant
//!
//! Path A downloads the UNCOMPENSATED reference. The device recomputes compensation
//! on-device ("Recalculate Compensation Continuously ON"); if the host ALSO
//! compensated the downloaded reference the pulse would be DOUBLE-COMPENSATED and
//! over-stroke the shaker. The type seam [`ReferenceForPath`] makes that
//! unrepresentable at compile time: the delegated constructor
//! ([`ReferenceForPath::delegated`]) will only accept a nonnegative (one-signed)
//! plain pulse, and it yields an [`UncompensatedReference`] newtype the delegated fire
//! path consumes; the host constructor ([`ReferenceForPath::host`]) accepts an
//! already-compensated waveform (which by construction has negative shoulders). See
//! the type docs for the enforced contract and the fixture in `tests/`.

use serde::{Deserialize, Serialize};

/// Which drive path a shock test runs on. This is the ONE discriminant the whole
/// module keys capability decisions off.
///
/// It is `Copy` and serializes, so a service can put it in its run config and the UI
/// can round-trip it. The capability table [`PathCapabilities::for_path`] maps it to
/// the feature-availability booleans; nothing outside this module should hard-code
/// "delegated => greyed" logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DrivePath {
    /// Path A: a delegated controller closes the loop on-device. Eigen downloads
    /// a reference and fires absolute setpoints; the device owns compensation, system
    /// ID, and control.
    Delegated,
    /// Path B: Eigen drives the DAC directly and closes the whole loop on the host.
    /// Compensation, synthesis, limiting, and the control law are Eigen's to compute.
    HostDac,
}

impl DrivePath {
    /// True for [`DrivePath::Delegated`].
    #[inline]
    pub fn is_delegated(self) -> bool {
        matches!(self, DrivePath::Delegated)
    }

    /// True for [`DrivePath::HostDac`].
    #[inline]
    pub fn is_host(self) -> bool {
        matches!(self, DrivePath::HostDac)
    }

    /// The capability table for this path. Convenience wrapper
    /// over [`PathCapabilities::for_path`].
    #[inline]
    pub fn capabilities(self) -> PathCapabilities {
        PathCapabilities::for_path(self)
    }
}

/// The single source of truth for which shock features are available on a given drive
/// path. Services and the UI read these
/// booleans instead of re-deriving path logic, so "greyed on Path A" is decided in
/// exactly one place.
///
/// Every field is documented with the reason that pins its value. The table is a
/// pure function of the [`DrivePath`] ([`PathCapabilities::for_path`]); it is
/// serializable only so a snapshot can record what the run believed was available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathCapabilities {
    /// Host-side compensation is available.
    ///
    /// **Path A: false.** The device compensates on-device; host
    /// compensation would DOUBLE-COMPENSATE and over-stroke. The delegated path
    /// downloads the UNCOMPENSATED reference (enforced by [`ReferenceForPath`]).
    /// **Path B: true.** Eigen owns compensation (the host compensation library).
    pub host_compensation: bool,

    /// Shaped system identification is available.
    ///
    /// **Path A: false.** The device owns system ID at arm; Eigen only
    /// observes. **Path B: true.** Eigen can drive a shaped low-level burst and
    /// estimate the plant FRF for drive prediction.
    pub shaped_sysid: bool,

    /// At-rate pulse trains (gunfire rounds/min, Procedure IV) are
    /// available.
    ///
    /// **Path A: false.** At-rate trains are HostDac-only: on the
    /// delegated path each pulse triggers an on-device compensation recompute (OEM
    /// pulses were 22-92 s apart) and the write-triggers-fire semantic is a gating
    /// unknown (hardware-blocked). **Path B: true** (Sim-demonstrable
    /// until direct-DAC lands).
    pub pulse_trains_at_rate: bool,

    /// Drive voltage / current limiting is OBSERVABLE.
    ///
    /// **Path A: false.** V/I drops are unobservable through the
    /// delegated path; the ceilings are greyed. **Path B: true.** It was confirmed
    /// (2026-07-04) that the amplifier V/I drops are wired into the CHANNEL TABLE as
    /// ordinary channels (Voltage/Current units, standard mV-per-EU sensitivity), so
    /// the between-pulse measured enforcement engine can read them. (The per-channel
    /// peak CEILINGS still live in the test config, not the channel identity - the
    /// identity-vs-test scope line.)
    pub vi_limiting_observable: bool,

    /// Eigen's own closed-loop control law runs.
    ///
    /// **Path A: false.** The device closes the loop; Eigen's [`ShockControlLaw`]
    /// ([`crate::control_law::ShockControlLaw`]) does not run.
    /// **Path B: true.** The per-band gain law and predictive-notch limiting run on
    /// the host.
    pub eigen_control_law: bool,
}

impl PathCapabilities {
    /// The capability table for `path`. This is THE mapping;
    /// treat it as authoritative.
    pub const fn for_path(path: DrivePath) -> Self {
        match path {
            // Path A: the device owns everything closed-loop; Eigen observes. Every
            // host-computed feature resolves to "false".
            DrivePath::Delegated => PathCapabilities {
                host_compensation: false,
                shaped_sysid: false,
                pulse_trains_at_rate: false,
                vi_limiting_observable: false,
                eigen_control_law: false,
            },
            // Path B: Eigen owns the whole loop on the host.
            DrivePath::HostDac => PathCapabilities {
                host_compensation: true,
                shaped_sysid: true,
                pulse_trains_at_rate: true,
                vi_limiting_observable: true,
                eigen_control_law: true,
            },
        }
    }
}

/// A reference waveform tagged by the path it is legal on. This is the type seam that
/// makes the double-compensation bug unrepresentable.
///
/// # Why a tagged enum and not a bare `Vec<f64>`
///
/// The whole point is that a delegated (Path A) reference download MUST be the
/// UNCOMPENSATED pulse and a host (Path B) reference is (or may be) compensated. If
/// both were "just a waveform" a caller could hand a compensated waveform to the
/// delegated fire path and silently over-stroke the shaker. By constructing the
/// reference through [`ReferenceForPath::delegated`] / [`ReferenceForPath::host`], the
/// path is baked into the value and the delegated fire path only accepts the
/// [`UncompensatedReference`] the delegated constructor yields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ReferenceForPath {
    /// A Path-A (delegated) reference: the UNCOMPENSATED, one-signed pulse the device
    /// downloads and then compensates on-device.
    Delegated(UncompensatedReference),
    /// A Path-B (host) reference: the waveform Eigen drives directly, which MAY carry
    /// host-computed compensation shoulders (and so may go negative).
    Host {
        /// The (possibly already-compensated) acceleration waveform, g.
        waveform_g: Vec<f64>,
    },
}

/// The maximum negative fraction of the peak a "nonnegative" delegated pulse may dip
/// to and still be accepted, absorbing float noise and the tiny undershoot a
/// bandlimited generator can leave at the endpoints. A genuine compensation shoulder
/// (e.g. -24% trough) is far below this and is rejected.
const DELEGATED_NEG_TOLERANCE_FRAC: f64 = 0.02;

impl ReferenceForPath {
    /// Build a DELEGATED (Path A) reference from a plain, UNCOMPENSATED pulse
    /// (the device compensates on-device; the host must NOT).
    ///
    /// The invariant checked here is that the pulse is one-signed (nonnegative): a
    /// classical delegated reference is a plain half-sine / sawtooth / etc. with NO
    /// compensation shoulders, so it never dips meaningfully below zero. A waveform
    /// with a compensation trough (negative shoulders) is a Path-B artifact and is
    /// REJECTED here with [`ReferenceError::CompensatedOnDelegatedPath`] - handing a
    /// compensated waveform to the delegated path is exactly the double-compensation
    /// bug this seam exists to prevent.
    ///
    /// A small negative tolerance ([`DELEGATED_NEG_TOLERANCE_FRAC`] of the peak)
    /// absorbs float noise / bandlimited endpoint undershoot; a real compensation
    /// shoulder is far below it. An empty or all-zero waveform is rejected as
    /// [`ReferenceError::EmptyReference`].
    pub fn delegated(uncompensated_pulse_g: &[f64]) -> Result<Self, ReferenceError> {
        if uncompensated_pulse_g.is_empty() {
            return Err(ReferenceError::EmptyReference);
        }
        let peak = uncompensated_pulse_g
            .iter()
            .fold(0.0_f64, |m, &v| m.max(v.abs()));
        if peak <= 0.0 {
            return Err(ReferenceError::EmptyReference);
        }
        let most_negative = uncompensated_pulse_g.iter().fold(0.0_f64, |m, &v| m.min(v));
        // A one-signed pulse may dip only within float/endpoint tolerance below zero.
        if most_negative < -DELEGATED_NEG_TOLERANCE_FRAC * peak {
            return Err(ReferenceError::CompensatedOnDelegatedPath {
                most_negative_g: most_negative,
                peak_g: peak,
            });
        }
        Ok(ReferenceForPath::Delegated(UncompensatedReference {
            pulse_g: uncompensated_pulse_g.to_vec(),
        }))
    }

    /// Build a HOST (Path B) reference from an (already-compensated or plain)
    /// waveform. No sign constraint: a host reference may carry compensation shoulders
    /// and go negative.
    pub fn host(waveform_g: &[f64]) -> Result<Self, ReferenceError> {
        if waveform_g.is_empty() {
            return Err(ReferenceError::EmptyReference);
        }
        Ok(ReferenceForPath::Host {
            waveform_g: waveform_g.to_vec(),
        })
    }

    /// The drive path this reference is legal on.
    pub fn path(&self) -> DrivePath {
        match self {
            ReferenceForPath::Delegated(_) => DrivePath::Delegated,
            ReferenceForPath::Host { .. } => DrivePath::HostDac,
        }
    }

    /// The underlying waveform (g), regardless of path. For a delegated reference this
    /// is the uncompensated pulse; for a host reference it is whatever waveform was
    /// supplied.
    pub fn waveform_g(&self) -> &[f64] {
        match self {
            ReferenceForPath::Delegated(u) => &u.pulse_g,
            ReferenceForPath::Host { waveform_g } => waveform_g,
        }
    }
}

/// The UNCOMPENSATED pulse a delegated (Path A) reference download carries. A newtype
/// so the delegated fire path's signature can DEMAND an uncompensated reference and
/// the type system rules out a compensated one.
///
/// Construct it only via [`ReferenceForPath::delegated`], which enforces the
/// one-signed invariant; there is no public field-struct constructor, so a
/// compensated waveform cannot be smuggled in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UncompensatedReference {
    /// The plain, one-signed acceleration pulse, g. The device compensates this
    /// on-device; the host must not.
    pulse_g: Vec<f64>,
}

impl UncompensatedReference {
    /// The uncompensated pulse waveform, g.
    pub fn pulse_g(&self) -> &[f64] {
        &self.pulse_g
    }
}

/// Errors from constructing a [`ReferenceForPath`].
#[derive(Debug, Clone, PartialEq)]
pub enum ReferenceError {
    /// The waveform was empty or all-zero.
    EmptyReference,
    /// A compensated waveform (negative shoulders beyond tolerance) was handed to the
    /// DELEGATED path constructor. This is the double-compensation bug the seam prevents:
    /// the device compensates on-device, so the download must be uncompensated.
    CompensatedOnDelegatedPath {
        /// The most-negative sample found (g).
        most_negative_g: f64,
        /// The pulse peak (g), for context.
        peak_g: f64,
    },
}

impl std::fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReferenceError::EmptyReference => {
                write!(f, "reference waveform is empty or all-zero")
            }
            ReferenceError::CompensatedOnDelegatedPath {
                most_negative_g,
                peak_g,
            } => write!(
                f,
                "compensated waveform (min {most_negative_g:.4} g vs peak {peak_g:.4} g) handed to the \
                 delegated path; the delegated download MUST be the uncompensated pulse \
                 or the device double-compensates and over-strokes"
            ),
        }
    }
}

impl std::error::Error for ReferenceError {}
