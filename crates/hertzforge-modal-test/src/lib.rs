//! Modal Test: experimental modal acquisition by hammer impact or by open-loop shakers.
//!
//! * [`setup`]: the setup, its value checks and refusals by name.
//! * [`plan`]: the measurement model and the roving plan, checked against the table.
//! * [`window`]: the force and exponential windows.
//! * [`hits`]: the checks every hit takes.
//! * [`source`]: the shaker sources and their feeder thread.
//! * [`average`]: the averages per measurement and their estimates.
//! * [`session`]: the state machine the run's thread drives.
//! * [`host`]: the station, the engine and the pump in both modes.
//! * [`structure`]: the simulated structure and hammer.

#![forbid(unsafe_code)]

pub mod average;
pub mod hits;
pub mod host;
pub mod plan;
pub mod session;
pub mod setup;
pub mod source;
pub mod structure;
pub mod window;
