//! `eigen-random` - the Single-Output Random control core and run service.
//!
//! This crate is the closed-loop control math for random vibration: it graduates
//! the converged `RandomController` from `crates/eigen-demo` into a reusable,
//! testable control law behind a trait, and adds the production features the demo
//! never had - measurement trust gating, per-iteration correction clamping,
//! control-channel compositing, response/force notch limiting, and divergence
//! accounting.
//!
//! # Scope (control core + run service)
//!
//! [`control`] is the CONTROL CORE: the [`control::ControlLaw`] trait, the one law
//! shipped now ([`control::HinvLaw`]), the control composite
//! ([`control::composite_psd`]), the notch limiter ([`control::NotchSet`]), and the
//! divergence bookkeeping. It is pure math over per-loop PSD/FRF/coherence inputs.
//!
//! [`service`] is the RUN SERVICE: [`service::RandomService`] orchestrates a full
//! SISO random run against the Sim plant (Path B) - the level ladder state machine
//! (feasibility -> sysID -> coherence gate -> engage -> level ladder -> full ->
//! duration -> stop), the alarm/abort envelope engine, the reactive kinematic monitor,
//! the drive-Vrms clamp/abort, the divergence abort, the overload proxy, the capture
//! rules, periodic save, and the serde report + status surface. It closes the loop
//! around the control core exactly as the adversarial fixtures do.
//!
//! # Two hardware paths - which one this crate serves
//!
//! Eigen has two random paths. **Path A (delegated)**: the DEVICE closes
//! the control loop internally; the host downloads a reference and schedules level
//! nudges. **Path B (host-closes-loop)**: the HOST runs its own control law over
//! measured PSD/FRF each loop. **Every law in this crate is Path B only.** On the
//! delegated path these laws do NOT run - the device owns the loop and Eigen's
//! control math is bypassed. A UI must grey the control-law knobs when the active
//! path is delegated so it makes no silent promise of Eigen control on a
//! device-closed loop.
//!
//! # Honesty posture
//!
//! Nothing in this crate claims hardware. The laws run against the `eigen-hal-sim`
//! synthetic plant today (the adversarial fixtures and the demo-parity gate); the real-
//! hardware Path B homes are a direct-DAC route and NI hardware, both gated
//! on separate work. This is control math validated
//! in simulation, not a hardware bring-up.
//!
//! Pure library. No `unwrap`/`expect`/`panic!` in library paths (validation returns
//! typed errors or is expressed as clamps/holds); the only asserts are in tests.
#![forbid(unsafe_code)]

pub mod control;
pub mod service;

pub use control::{
    composite_psd, CompositeMode, ControlError, ControlLaw, HinvConfig, HinvLaw, LoopDiagnostics,
    LoopInputs, LoopOutputs, NotchReport, NotchSet,
};

pub use service::config::{
    CaptureRules, CaptureTrigger, ConfigError, ControlChannel, ControlSet, LevelSchedule,
    LimitKind, LimitProfile, MetaField, RandomTestConfig, RunMode,
};
pub use service::engine::{
    GridRefusal, GridSnapshot, KinematicMonitor, LoopConfig, StationAdoption, MIN_CONTROL_LINES,
};
pub use service::report::{ReportData, RunState, Status, TripKind, TripRecord};
pub use service::{RandomService, RandomSnapshotPayload, ServiceError};
