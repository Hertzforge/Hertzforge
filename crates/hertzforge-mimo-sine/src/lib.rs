//! MIMO Sine: sweeps, dwells and stepped sines at several control channels
//! with several drives, one tone at a time, closed every control update.
//!
//! One tone is played on every drive, each with its own amplitude and phase,
//! and tracked on every control and limit channel. Each update the core reads
//! the blocks, updates every tracking filter, judges the controls and limits,
//! steps the schedule (frequency and level), calls the law, guards its answer,
//! ramps each drive to the guarded value and records.
//!
//! This crate holds the contracts:
//!
//! - [`limits`]: the size limits and default guards of the first version.
//! - [`setup`]: the run's setup ([`MimoSineSetup`]: controls with their
//!   references and bands, drives with their ceilings, limit channels with
//!   their bounds, the test and its level schedule, tracking, identification,
//!   the law, requests and guards), with validation.
//! - [`tick`]: what a law sees on one update ([`TickInputs`]) and returns
//!   ([`TickOutputs`]), with the answer's checks.
//! - [`law`]: what a law is built with ([`LawSetup`]), its manifest and the
//!   native law traits ([`MimoSineLaw`], [`MimoSineLawFactory`]).
//! - [`engine`]: the run engine's public surface. Its run functions are not
//!   built in this version and answer [`EngineError::NotBuilt`].
//!
//! The law file world is `wit/control-law/sine-mimo.wit`; its records mirror
//! these types field for field.

pub mod engine;
pub mod law;
pub mod limits;
pub mod setup;
pub mod tick;

pub use engine::{EngineError, MimoSineEngine};
pub use law::{
    check_law_accepts, LawIdentification, LawManifest, LawSetup, LawSetupError, LawSetupRule,
    LawTolerance, MimoSineLaw, MimoSineLawFactory, BUILTIN_LAW_IDS, DEFAULT_LAW_ID,
};
pub use limits::{MAX_CONTROLS, MAX_DRIVES, MAX_LAW_VALUES, MAX_LIMITS};
pub use setup::{
    shape_name, ControlChannel, DriveChannel, GuardSettings, IdentificationSettings, LawChoice,
    LevelMode, LimitChannel, MimoSineSetup, NotchSettings, PhasePoint, RequestSettings, Schedule,
    SetupError, SetupRule, StepPlan, TestPlan, TestType, Tracking,
};
pub use tick::{check_outputs, Hold, LawFault, OutputRule, Request, TickInputs, TickOutputs};

pub use hertzforge_sdm::law::MimoShape;

#[cfg(test)]
mod tests;
