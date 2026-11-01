//! `hertzforge-srs` - SRS SYNTHESIS engines + predictive-notch response limiting
//! of the Shock build.
//!
//! Two synthesis engines drive a shaker to match a target Shock Response Spectrum
//! (SRS), plus the component tables that parameterize them, the SRS-matching outer
//! loop, the Coleman/Smallwood predictive-notch limiter, and the run-time
//! control-law counterpart:
//!
//! - [`wavsyn`] - WAVSYN odd-half-cycle wavelets. Each component
//!   `W_m(t) = A_m sin(2*pi*f_m*t/N_m) sin(2*pi*f_m*t)` on `[0, N_m/(2*f_m)]` with an
//!   ODD number of half-cycles `N_m` has ZERO net velocity AND zero net displacement
//!   BY CONSTRUCTION; the composite (a delayed sum) inherits that property, so no
//!   compensation is ever needed. Reverse-sine-sweep default delays.
//! - [`seds`] - SEDS sums of exponentially-decaying sinusoids
//!   `a(t) = sum A_m exp(-zeta_m*w_m*(t-t_m)) sin(w_m*(t-t_m))`. A raw damped sine does
//!   NOT self-null (nonzero net velocity and displacement), so a Smallwood-Nord-style
//!   compensation set is superposed to drive the COMPOSITE net velocity and
//!   displacement to ~0. Cites Smallwood & Nord 1974 and Nelson & Prasthofer 1974.
//! - [`table`] - the [`table::WaveletComponent`] / [`table::DampedSineComponent`]
//!   component tables: defaults of one component per fractional-octave band, hardened
//!   TSV/CSV/clipboard import (header aliasing, RFC-4180, comment tolerance) mirroring
//!   `hertzforge-breakpoints`, and serde JSON round-trip.
//! - [`iterate`] - the SRS-matching outer loop [`iterate::synthesize_to_target`] and
//!   its capped variant [`iterate::synthesize_to_target_capped`]:
//!   score with [`hertzforge_dsp::srs::srs_analyze`], adjust each component's amplitude by
//!   its nearest-band target/achieved ratio (clamped), re-synthesize, repeat. Works
//!   when the component count differs from the analysis-band count (gate
//!   iv). The capped variant honors per-component amplitude ceilings from the
//!   predictive notch (limit-down beats control-up, the Random arbitration rule).
//! - [`limit`] - predictive-notch response limiting (Skousen/Coleman/Smallwood,
//!   SAND2016-1335C): [`limit::LimitChannel`] non-control locations with
//!   reduced-level FRFs and SRS bounds, and [`limit::PredictiveNotch`], which
//!   predicts each limit channel's response SRS from the drive spectrum, notches the
//!   drive in the DRIVE-SPECTRUM domain where predictions exceed bounds, and
//!   projects the notch back to per-component gains by a LEAST-SQUARES re-fit
//!   (three distinct grids, no per-line copy).
//! - [`law`] - [`law::SrsSynthesisLaw`], the run-time counterpart implementing
//!   [`hertzforge_shock_common::ShockControlLaw`]: the same per-band ratio update applied
//!   pulse-by-pulse from the measured SRS. Built [`law::SrsSynthesisLaw::with_limits`],
//!   it folds the predictive-limit cap from `predicted_limit_srs` into every gain
//!   update (wired).
//! - [`preview`] - [`preview::synthesize_preview`], the OFFLINE preview: the same
//!   seed table and the same iteration the service runs, called directly on an
//!   [`service::config::SrsTestConfig`] with an explicit rate, so a target can be
//!   authored and its synthesized pulse looked at with no plant, no station and no
//!   run (an operator acceptance request). A preview is a COMPUTATION, never a
//!   measurement - see that module's provenance note.
//! - [`service`] - [`service::SrsShockService`], the run orchestration on top
//!   of all of the above (state machine mirroring Random/SMART, per-pulse
//!   fire -> capture -> SRS -> law pipeline, the log-octave tolerance envelope,
//!   deterministic feasibility gate, reduced-level system ID,
//!   level ladder, drive limiting, capture/snapshots, report + status).
//!   Path B closes the loop on the host; the delegated path is reference-download
//!   + absolute-scalar fires with NO Hertzforge control law on the device.
//!
//! # What is deliberately NOT here
//!
//! Tolerance-band checking is `hertzforge-shock-host`; the FRF engine and the SRS
//! analysis primitives stay in `hertzforge-dsp`; drive peak V/I/accel limiting (the
//! honesty split) is `hertzforge_shock_common::drive_limit` (shared by both shock
//! modules).
//!
//! # Determinism
//!
//! Every function here is deterministic: no RNG, no dither. The synthesized waveforms
//! are a pure function of the component table and sample rate, so a golden waveform is
//! reproducible bit-for-bit and the gates can assert exact residuals.
//!
//! # Units
//!
//! Amplitudes and SRS are in g; time in seconds; frequency in Hz. Kinematic
//! verification runs through [`hertzforge_dsp::shock::pulse_kinematics`], which reports the
//! in/g system (velocity in in/s, displacement in inches) via
//! [`hertzforge_dsp::integrate::G_TO_IN_S2`].
#![allow(dead_code)]

pub mod iterate;
pub mod law;
pub mod limit;
pub mod preview;
pub mod seds;
pub mod service;
pub mod table;
pub mod wavsyn;

// Curated re-exports so downstream crates and the UI bind against one path.
pub use iterate::{
    synthesize_to_target, synthesize_to_target_capped, SynthesisEngine, SynthesisOpts,
    SynthesisResult,
};
pub use law::{LawLimitStatus, LawLimits, SrsSynthesisLaw};
pub use limit::{
    predict_response_srs, predict_response_waveform, LimitChannel, NotchError, NotchOutcome,
    NotchReport, NotchedBand, PredictiveNotch,
};
pub use preview::{analyze_srs, synthesize_preview, PreviewKinematics, SynthesisPreview};
pub use seds::{
    seds_compensated, seds_composite_raw, CompensationReport, SedsCompensation, SedsSynthResult,
};
pub use service::config::SrsTestConfig;
pub use service::plant::{DelegatedShockDevice, FirePath, HalPulsePlant, PulsePlant};
pub use service::{ServiceError, SrsShockService};
pub use table::{
    ComponentError, DampedSineComponent, DampedSineTable, WaveletComponent, WaveletTable,
};
pub use wavsyn::{wavsyn_component, wavsyn_composite};

#[cfg(test)]
mod tests;
