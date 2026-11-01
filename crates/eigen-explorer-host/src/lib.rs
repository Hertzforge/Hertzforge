//! `eigen-explorer-host` - the DEVICE-NEUTRAL half of the Explorer/SMART module.
//!
//! ## Why this crate exists
//!
//! The Explorer module has two drive paths. On a delegated controller the device runs
//! the sine and the host decides when to send it nudges; that service links the
//! delegated backend and lives with it, outside the open set. A host-closes-loop
//! backend (NI today) can never link a crate that drags a delegated USB backend in
//! with it.
//!
//! So the pieces of the Explorer that never needed the device at all live here:
//!
//! * [`analysis`] - spectra, resonance profiles, derived velocity/displacement. Already
//!   sample-rate-parameterized; moved verbatim.
//! * [`guards`] - the drive-domain types, the predictive band/ceiling guard, and the
//!   reactive [`guards::AbortMonitor`], the last with its interleave stride AND its slot
//!   derivation parameterized instead of hard-wired to an 8-channel frame.
//! * [`floor`] - the measured-noise-floor probe, generalized from a fixed-size array to
//!   a runtime interleave width.
//! * [`slew`] - the operator slew ceilings, restated PER FRAME (see [`slew::SlewLimits`]
//!   for why this is a different type from the pacer's same-named one).
//! * [`capture`] - the snapshot payload the capture infrastructure carries.
//! * [`freq`] - the windowed zero-crossing frequency MEASUREMENT, so the
//!   delegated path's estimate correction and the host path's commanded-vs-measured
//!   cross-check are the same measurement).
//!
//! ## The control core
//!
//! The half that makes this crate a MODULE rather than a toolbox:
//!
//! * [`step`] - [`ExplorerProcessStep`](step::ExplorerProcessStep), the persistent
//!   [`eigen_engine::ProcessStep`] that synthesizes a phase-continuous, slew-limited,
//!   volts-native interactive sine and taps the acquisition into a ring.
//! * [`ring`] - the bounded, drop-oldest, recycling ring that gets the display FFT off
//!   the RT tick.
//! * [`service`] - [`ExplorerHostService`](service::ExplorerHostService), the run
//!   orchestration: adopt-the-actual-rate, the arm gate, the 0.000 V arm posture, the
//!   bounded tick batches under `catch_unwind`, and the off-tick analysis pass.
//! * [`host`] - [`RunHost`](host::RunHost), the pump thread around the service: the
//!   asynchronous arm behind a priming flag, the lock-free status snapshot, the stall
//!   watchdog and the abort tail, with a [`HostSink`](host::HostSink) for what each front
//!   end does with the display output.
//!
//! ## The firewall
//!
//! This crate MUST NOT depend on a concrete hardware backend. `cargo deny check bans`
//! enforces it: the delegated backend is banned workspace-wide and this crate is not on
//! the wrapper whitelist. The control was RUN (a planted backend dependency here failed
//! `check bans`, then was reverted), because a firewall that has never been observed
//! failing is not a firewall.
//!
//! ## What is not here
//!
//! Anything that names a delegated-backend type stays with the delegated Explorer
//! service: the nudge vocabulary, the link trait, the estimators, the pacer, the sweep
//! scheduler, the compress ramp, the service, and the two nudge-step methods on
//! [`guards::PredictiveGuard`] (an extension trait there). That service re-exports
//! everything in this crate at its original paths.

pub mod analysis;
pub mod capture;
pub mod floor;
pub mod freq;
pub mod guards;
pub mod host;
pub mod level;
pub mod profile_raster;
pub mod ring;
pub mod service;
pub mod slew;
pub mod step;

pub use service::{
    ExplorerCommand, ExplorerHostConfig, ExplorerHostService, ExplorerStatus, RunState,
    ServiceError,
};
pub use step::{AmplitudeRamp, ExplorerProcessStep, SweepDirection, SweepState};
