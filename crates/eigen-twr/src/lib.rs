//! `eigen-twr` - Time Waveform Replication (TWR / Time History
//! Replication).
//!
//! Reproduces a recorded (or analytically specified) acceleration TIME HISTORY at
//! the control accelerometer: system-ID the plant FRF at low level, invert it with
//! regularization, and ITERATE a frequency-domain drive correction between plays
//! until the achieved response matches the target within tolerance
//! (MIL-STD-810 Method 525 shape).
//!
//! # The control law (implemented in [`law`])
//!
//! ```text
//! sysid:   H(f), gamma^2(f)  from low-level broadband ranging (Welch Hv)
//! inverse: G(f) = conj(H) / (|H|^2 + eps)      (Tikhonov; eps from beta_floor)
//! init:    D_0(f)     = G(f) * R_L(f)          (R_L = level-scaled target FFT)
//! iterate: D_{n+1}(f) = D_n(f) + beta * G(f) * (R_L(f) - Y_n(f))
//! gate:    rms_error_pct = 100 * ||y - r_L|| / ||r_L||  <= gate  -> next level
//! ```
//!
//! per-bin trust gating (coherence floor + reference dynamic-range floor + band
//! mask) and a per-iteration correction clamp bound the update everywhere the
//! estimate cannot be trusted.
//!
//! # Module map
//!
//! - [`reference`] - target waveform: import (CSV/TSV/WAV/JSON), prep ops,
//!   kinematics preview.
//! - [`grid`] - the padded working grid + half-spectrum FFT helpers (the
//!   wraparound pad).
//! - [`sysid`] - ranging excitation (band-limited COLA random or log chirp) + the
//!   Hv/coherence estimate + the coherence hold gate.
//! - [`inverse`] - regularized inverse on the sysid grid + interpolation onto the
//!   record grid + the per-bin trust mask.
//! - [`law`] - the pure iterative correction law (unit-testable with no backend).
//! - [`metrics`] - error metrics + convergence history + debounced divergence
//!   detection.
//! - [`schedule`] - the convergence-gated dB ladder + the run schedule
//!   (once / loop N / duration; section from-to).
//! - [`limits`] - response abort envelope, drive clip/RMS ceilings, feasibility
//!   hard-block + override-ack.
//! - [`player`] - the Backend-agnostic drive-delivery seam (direct
//!   Backend playout, NOT through `eigen_engine::Engine`).
//! - [`service`] - the run orchestration state machine + status/report surface.

pub mod grid;
pub mod inverse;
pub mod law;
pub mod limits;
pub mod metrics;
pub mod player;
pub mod reference;
pub mod schedule;
pub mod service;
pub mod sysid;

pub use grid::RecordGrid;
pub use inverse::{GridInverse, InverseConfig};
pub use law::LawConfig;
pub use limits::{FeasibilityCeilings, FeasibilityReport, TwrLimits};
pub use metrics::{ConvergenceHistory, ConvergenceRecord, ErrorMetrics};
pub use player::{BackendReferencePlayer, ReferencePlayer, TwrPath};
pub use reference::{ReferenceWaveform, WaveformError};
pub use schedule::{RunSchedule, Section, TwrLadderState, TwrLevelSchedule};
pub use service::config::TwrConfig;
pub use service::{TwrService, TwrState, TwrStatus};
pub use sysid::{Excitation, SysIdConfig, SysIdResult};

/// The module-wide error type.
#[derive(Debug, Clone, PartialEq)]
pub enum TwrError {
    /// Reference import / preparation problem.
    Waveform(String),
    /// Configuration invalid (message names the field).
    Config(String),
    /// System identification failed (low coherence, playout error, ...).
    SysId(String),
    /// The drive-delivery path failed (backend error, no response record, ...).
    Play(String),
    /// A limit tripped or a guard aborted the run. Carries the reason.
    Aborted(String),
    /// An operation was asked of a state that does not allow it.
    State(String),
    /// A play was CANCELLED at a block boundary by the operator's stop/abort.
    ///
    /// Distinct from [`TwrError::Aborted`] on purpose: an abort is a guard
    /// tripping and ends the run `Aborted`, a cancel is the operator asking and
    /// ends it `Stopped` (the run that reported ABORTED with an unintelligible
    /// stream-error string when the operator pressed Stop). The frames
    /// are what the DEVICE had accepted when the flag was seen; what it already
    /// holds keeps playing until the HAL's stop taper brings it to rest.
    Cancelled {
        frames_done: usize,
        frames_expected: usize,
    },
}

impl std::fmt::Display for TwrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TwrError::Waveform(s) => write!(f, "waveform: {s}"),
            TwrError::Config(s) => write!(f, "config: {s}"),
            TwrError::SysId(s) => write!(f, "sysid: {s}"),
            TwrError::Play(s) => write!(f, "play: {s}"),
            TwrError::Aborted(s) => write!(f, "aborted: {s}"),
            TwrError::State(s) => write!(f, "state: {s}"),
            TwrError::Cancelled {
                frames_done,
                frames_expected,
            } => write!(
                f,
                "cancelled: operator stop after {frames_done} of {frames_expected} frames"
            ),
        }
    }
}

impl std::error::Error for TwrError {}
