//! `hertzforge-explorer-host` - Explorer, the hand-steered open-loop sine.
//!
//! The operator steers one tone by hand: its frequency, its level in volts peak at the
//! drive output, and a sweep toward either band edge. Nothing closes a loop: the run
//! measures the response and never corrects the drive from it. The pieces:
//!
//! * [`step`] - [`ExplorerProcessStep`](step::ExplorerProcessStep), the persistent
//!   [`hertzforge_engine::ProcessStep`] that synthesizes a phase-continuous, slew-limited,
//!   volts-native sine, checks every sample against the ceiling, and taps the
//!   acquisition into a ring.
//! * [`slew`] - the operator's slew ceilings, restated per frame at the adopted rate.
//! * [`ring`] - the bounded, drop-oldest, recycling ring that gets the analysis off the
//!   tick.
//! * [`service`] - [`ExplorerHostService`](service::ExplorerHostService), the run:
//!   adopt the rate the station programmed, the arm gate, the 0.000 V arm posture, the
//!   bounded tick batches under `catch_unwind`, and the off-tick analysis pass.
//! * [`intent`] - what the operator asked and what the run did with it, placed in the
//!   run's own time.
//! * [`host`] - [`RunHost`](host::RunHost), the pump thread around the service: the
//!   asynchronous arm, the lock-free status snapshot, the stall watchdog and the abort
//!   tail, with a [`HostSink`](host::HostSink) for what a front end does with the output.
//! * [`analysis`] - the display spectra, the survey capture, the derived velocity and
//!   displacement; [`profile_raster`] - the survey line itself; [`freq`] - the measured
//!   frequency; [`floor`] - the noise-floor probe; [`guards`] - the secondary limit
//!   monitor.
//!
//! This crate names no hardware backend: it is handed a finished station backend, and
//! `cargo deny check bans` keeps a concrete backend out of it.

pub mod analysis;
pub mod floor;
pub mod freq;
pub mod guards;
pub mod host;
pub mod intent;
pub mod profile_raster;
pub mod ring;
pub mod service;
pub mod slew;
pub mod step;

pub use intent::{
    Adoption, FrequencyPart, IntentKind, IntentRecord, LevelPart, Steered, StopCause,
};
pub use service::{
    DriveTrip, EndRecord, ExplorerCommand, ExplorerHostConfig, ExplorerHostService, ExplorerStatus,
    RunState, ServiceError,
};
pub use step::{AmplitudeRamp, ExplorerProcessStep, SweepDirection, SweepState};
