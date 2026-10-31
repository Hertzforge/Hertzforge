//! `sds-control`: the control service of the structural dynamics suite.
//!
//! It runs the controller's random control engine (`eigen-random`) closing its loop
//! around the controller's Sim backend (`eigen-hal-sim`), and serves the `sds/1`
//! WebSocket protocol (`schema/PROTOCOL.md`) on loopback:
//!
//! - [`codec`]: text and binary frame encoding and decoding, pinned to the golden
//!   frames in `schema/golden/`.
//! - [`protocol`]: message envelopes, error codes, request parsing.
//! - [`views`]: the viz kit's `spectrum` and `reading` views.
//! - [`profiles`]: demo random profiles defined by breakpoints, with their Sim plants.
//! - [`sim`]: one simulated run on its own thread, paced or unpaced.
//! - [`server`]: the WebSocket service.
//!
//! This crate drives no hardware. Its only backend is the Sim plant, every run it
//! reports carries `source: "SIM"`, and it depends on none of the controller's
//! private backend crates (`tests/forbidden_deps.rs`).
#![forbid(unsafe_code)]

pub mod codec;
pub mod profiles;
pub mod protocol;
pub mod server;
pub mod sim;
pub mod views;

/// Default bind address.
pub const DEFAULT_HOST: &str = "127.0.0.1";
/// Default port.
pub const DEFAULT_PORT: u16 = 8792;
