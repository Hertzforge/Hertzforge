//! `eigen-shock-common` - the pulse/transient run infrastructure shared by the two
//! shock modules (classical shock + SRS).
//!
//! This crate is the type seams + shared scaffolding and the deterministic-kinematics
//! feasibility path of the Shock build.
//!
//! # What lives here (the tight cut)
//!
//! - [`path`] - path-honesty type seams: [`path::DrivePath`] +
//!   [`path::PathCapabilities`] so services and the UI grey features
//!   from ONE source of truth, and the [`path::ReferenceForPath`] hard invariant that
//!   a delegated-path reference download is the UNCOMPENSATED pulse.
//! - [`control_law`] - the event-driven [`control_law::ShockControlLaw`] seam and
//!   the trivial [`control_law::PerBandGainLaw`] default (the classic SRS per-band
//!   ratio iteration). NOT the frozen plugin ABI.
//! - [`ladder`] - the [`ladder::ShockLevelSchedule`] state machine (mirrors
//!   eigen-random's ladder shape; N pulses per level instead of dwell).
//! - [`pulse_store`] - the per-pulse record store: [`pulse_store::PulseRecord`]
//!   computed on ingest via eigen-dsp, [`pulse_store::PulseStore`], the
//!   [`pulse_store::ShockSnapshotPayload`] for eigen-testkit's `Snapshot<P>`, and the
//!   capture wiring helper.
//! - [`train`] - the [`train::TrainSpec`] pulse-train scheduler + [`train::RateModel`]:
//!   the scaffolding, hardware-blocked markers included, and the gunfire
//!   Procedure-IV helper.
//! - [`feasibility`] - the DETERMINISTIC-KINEMATICS feasibility path:
//!   [`feasibility::pulse_feasibility`] reuses eigen-shaker-db's report/item/verdict
//!   types and consumes `force_shock` (which no existing code path consumes)
//!   like-for-like by its `RatingBasis`, plus the minimal
//!   [`feasibility::naive_double_sided_compensation`] helper (a placeholder).
//! - [`drive_limit`] - drive peak limiting with the HONESTY SPLIT: peak-ACCEL is
//!   PREDICTIVE pre-fire via the measured FRF
//!   ([`drive_limit::DriveLimit::pre_fire_accel`]); CURRENT/VOLTAGE ceilings are
//!   BETWEEN-PULSE measured enforcement on wired monitor channels
//!   ([`drive_limit::DriveLimit::between_pulse`]), inert/greyed on Path A where V/I
//!   are unobservable (gated by [`path::PathCapabilities::vi_limiting_observable`]).
//!   Generic over the channel's engineering unit (the Units-enum change is a
//!   separate deferred step).
//!
//! # What is deliberately NOT here
//!
//! Predictive-notch limiting and shaped-sysID orchestration are `eigen-srs` only;
//! tolerance-band checking is `eigen-shock-host`; the FRF engine stays in
//! `eigen-dsp` (it already exists there). This crate is the SHARED substrate the two
//! shock modules build on, nothing module-specific.
//!
//! Pure library plus filesystem I/O for the capture/snapshot path; no panics in
//! library paths (config validation returns errors).
#![allow(dead_code)]

pub mod control_law;
pub mod drive_limit;
pub mod feasibility;
pub mod ladder;
pub mod path;
pub mod pulse_store;
pub mod train;

// Curated re-exports so downstream crates and the UI bind against one path.
pub use control_law::{
    ControlLawError, PerBandGainLaw, ShockControlLaw, ShockLawInputs, ShockLawOutputs,
};
pub use drive_limit::{
    predict_peak_accel_g, BetweenPulseDecision, DriveLimit, DriveLimitError, DriveMonitor,
    DriveMonitorKind, MeasuredPeak, MonitorAction, MonitorEnforcement, PreFireDecision,
};
pub use feasibility::{naive_double_sided_compensation, pulse_feasibility, FeasibilityTolerances};
pub use ladder::{LadderError, ShockLadderState, ShockLevelSchedule, ShockRunMode};
pub use path::{DrivePath, PathCapabilities, ReferenceForPath, UncompensatedReference};
pub use pulse_store::{PulseRecord, PulseStore, ShockSnapshotPayload};
pub use train::{gunfire_spacing, RateModel, Spacing, TrainError, TrainSpec};

#[cfg(test)]
mod tests;
