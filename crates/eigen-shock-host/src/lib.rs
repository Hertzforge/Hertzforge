//! `eigen-shock-host` - the HOST-SIDE (Path B) classical-shock module.
//!
//! It holds everything the shock service needs that names no concrete backend, so
//! the daemon hosts can link the shock service WITHOUT pulling a delegated USB
//! backend into `eigen-backendd`'s graph. See this crate's `Cargo.toml` header for
//! the firewall argument in full.
//!
//! - [`spec`] - the classical pulse library spec: [`spec::ClassicalPulseSpec`]
//!   (shape + amplitude + duration + compensation config), main-pulse generation
//!   via `eigen_dsp::shock::pulse`, and Path-B synthesis via
//!   [`spec::ClassicalPulseSpec::synthesize`] (main pulse + host compensation
//!   through `eigen_dsp::shock::compensate_pulse`). Serde round-trips (the
//!   eigen-dsp compensation types are mirrored via serde remote derive because
//!   eigen-dsp is dependency-free by design).
//! - [`tolerance`] - the THREE tolerance checks: (a) the
//!   acceleration-amplitude envelope (per-standard bands around the nominal pulse,
//!   MIL-STD-810 516 / IEC 60068-2-27), (b) the velocity-change tolerance (the
//!   standards' dV acceptance, time-integral of the measured pulse vs nominal),
//!   and (c) the pulse-substitution dV-equivalence check plus the standalone
//!   [`spec::substitute_amplitude_for_velocity_change`] design helper.
//! - [`service`] - the CLASSICAL-SHOCK run service:
//!   [`service::ClassicalShockService`] orchestrates feasibility ->
//!   arm -> per-level firing (the level ladder) -> the per-pulse pipeline
//!   (fire -> record -> tolerance -> alarm/abort, drive limits) ->
//!   pulse trains + gunfire Procedure IV (hardware-blocked), with a
//!   timestamped event log, serde `ReportData`, honesty markers, and a
//!   `status()` snapshot for the UI.
//!
//! # The path-honesty split - read this before wiring a service
//!
//! - **Path A (Delegated):** download = the UNCOMPENSATED main pulse; the
//!   device compensates on-device ("Recalculate Compensation Continuously ON").
//!   Host compensation here would DOUBLE-COMPENSATE and over-stroke. That path
//!   lives with the delegated backend, outside this crate.
//! - **Path B (HostDac / Sim):** [`spec::ClassicalPulseSpec::synthesize`] is the
//!   compensated composite Eigen drives directly, through
//!   [`service::firer::BackendPulseFirer`] over any `eigen_hal::Backend`.
//!   `eigen_shock_common::PathCapabilities` is the one capability table;
//!   `host_compensation` is `false` on Path A and `true` on Path B.
//!
//! Units are the in/g/lbf system throughout.
#![allow(dead_code)]

pub mod service;
pub mod spec;
pub mod tolerance;

// Curated re-exports so downstream services and the UI bind against one path.
// The delegated shock crate mirrors this list verbatim, which keeps its import
// paths compiling.
pub use service::config::{CaptureRules, ClassicalTestConfig, ConfigError, GunfireSpec, MetaField};
pub use service::firer::{BackendPulseFirer, PulseFirer};
pub use service::report::{Honesty, PulseRow, ReportData, RunState, Status, TripKind, TripRecord};
pub use service::{ClassicalShockService, ServiceError};
pub use spec::{
    substitute_amplitude_for_velocity_change, ClassicalPulseSpec, ClassicalShape, SpecError,
};
pub use tolerance::{
    check_tolerance, EnvelopeCheck, SubstitutionCheck, ToleranceParams, ToleranceReport,
    ToleranceStandard, VelocityChangeCheck,
};
