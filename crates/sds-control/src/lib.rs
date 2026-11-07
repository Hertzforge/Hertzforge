//! `sds-control`: the control service of the structural dynamics suite.
//!
//! It runs the controller's closed-loop Random service (`hertzforge-random`) on the
//! simulator or on NI-DAQmx hardware, and serves the `sds/1` WebSocket protocol
//! (`schema/PROTOCOL.md`) on loopback:
//!
//! - [`codec`]: text and binary frame encoding and decoding, pinned to the golden
//!   frames in `schema/golden/`.
//! - [`protocol`]: message envelopes, error codes, request parsing.
//! - [`views`]: the viz kit's `spectrum` and `reading` views.
//! - [`profiles`]: demo random profiles defined by breakpoints, with their Sim plants.
//! - [`sim`]: one simulated run on its own thread, paced or unpaced (the `sim.*`
//!   methods, kept for the Simulate workspace's demonstrations).
//! - [`session`]: one controlling connection, observers, the lease and the orphan
//!   clock.
//! - [`station`]: the simulator and NI-DAQmx stations, built through the
//!   controller's station factory.
//! - [`table`]: the channel table and its validation.
//! - [`random`]: closed-loop Random through the controller's `RandomService`.
//! - [`mimo`]: MIMO Random: its setup, validation, driver trait and quantities.
//! - [`laws`]: the control-law catalog on the wire.
//! - [`server`]: the WebSocket service, loopback only.
//!
//! Hardware is reached only through `hertzforge-station`, the controller's one
//! construction point; a run's `source` is `HARDWARE` only on physical NI devices,
//! as the driver states them (`tests/forbidden_deps.rs` holds the dependency line).
#![forbid(unsafe_code)]

pub mod codec;
pub mod laws;
pub mod mimo;
pub mod profiles;
pub mod protocol;
pub mod random;
pub mod server;
pub mod session;
pub mod sim;
pub mod station;
pub mod table;
pub mod views;

/// Default bind address.
pub const DEFAULT_HOST: &str = "127.0.0.1";
/// Default port.
pub const DEFAULT_PORT: u16 = 8792;
