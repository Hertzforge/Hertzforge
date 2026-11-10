//! The Recorder through the protocol (schema/PROTOCOL.md, "Recorder"): the inputs
//! recorded with no drive. The configuration and its checks, the run around the core's
//! service, the state and live frames, the subscriptions, the run folder, and the
//! simulator's stated source.

pub mod config;
pub mod records;
pub mod run;
pub mod sim_signal;
pub mod subscribe;
pub mod view;
