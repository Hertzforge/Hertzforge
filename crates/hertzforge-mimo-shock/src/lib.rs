//! MIMO Shock: classical pulses and Shock Response Spectra (SRS) at several
//! control channels with several drives, closed between pulses.
//!
//! One pulse engine runs both target kinds ([`TargetKind`]): a compensated
//! waveform at every control, or an SRS at every control (with a waveform the
//! core synthesized for each). Per pulse the core builds and checks the drives,
//! fires them in one shot, measures every input, judges each control, ticks the
//! level ladder (which fixes the next level), calls the law with that level and
//! guards its answer.
//!
//! This crate holds the contracts:
//!
//! - [`limits`]: the size limits of the first version.
//! - [`setup`]: what a law is built for ([`Setup`]: [`Target`], [`Timing`],
//!   [`PlantStack`], each drive's ceiling, the ladder), with validation.
//! - [`pulse`]: what a law sees after a pulse ([`PulseInputs`]) and returns
//!   ([`PulseOutputs`]), with validation.
//! - [`law`]: the native law traits ([`MimoShockLaw`], [`MimoShockLawFactory`]).
//! - [`engine`]: the run engine's public surface (configuration, run states, the
//!   per-pulse report). Its run functions are not built in this version and
//!   answer [`EngineError::NotBuilt`].
//!
//! The law file world is `wit/control-law/shock-mimo.wit`; its records mirror
//! these types field for field. Records of several channels are flat, channel
//! after channel: sample `n` of channel `c` is `data[c * record_len + n]`.

pub mod builtin;
pub mod engine;
pub mod guards;
pub mod ident;
pub mod judge;
pub mod law;
pub mod limits;
pub mod pulse;
pub mod records;
pub mod setup;
pub mod targets;

pub use builtin::builtin_factory;
pub use engine::{
    ControlReport, DriveChannel, EndOutcome, EngineError, GuardPlan, IdentificationPlan,
    IdentificationReport, LadderPlan, LimitChannel, MimoShockConfig, MimoShockEngine, PulseReport,
    RunEnd, RunState, SeedReport, ShotError, ShotPath,
};
pub use law::{check_law_accepts, LawManifest, MimoShockLaw, MimoShockLawFactory};
pub use limits::{MAX_CONTROLS, MAX_DRIVES, MAX_LAW_VALUES, MAX_RECORD_LEN, MIN_RECORD_LEN};
pub use pulse::{
    ControlVerdict, InputsError, InputsRule, OutputError, OutputRule, PulseInputs, PulseOutputs,
};
pub use records::{replay, ReplayReport, RunRecord};
pub use setup::{
    ComponentShape, HoldTiming, PlantStack, Setup, SetupError, SetupRule, SrsComponent, SrsTarget,
    Target, TargetKind, Timing, WaveformTarget,
};
pub use targets::{ClassicalControl, SrsSpec, TargetSpec};

#[cfg(test)]
mod tests;
