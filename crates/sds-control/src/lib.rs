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
//! - [`shock`]: classical shock: its configuration, the reference pulse, the run
//!   on the transient-run rails ([`pulse`]) and its views.
//! - [`srs`]: SRS shock: its configuration, the offline synthesis and its preview,
//!   the imports, the run on the same rails and its views.
//! - [`sine`]: Sine: its configuration, the profile's imports and views, the
//!   run's state and live frames, its subscriptions and the law check's results.
//! - [`twr`]: time waveform replication: its references and their preview, the
//!   configuration, the run on the transient-run rails and its views, its
//!   subscriptions and the law check.
//! - [`explorer`]: Explorer, the hand-steered sine: its configuration and its
//!   subscriptions.
//! - [`recorder`]: the Recorder, the inputs recorded with no drive: its
//!   configuration.
//! - [`laws`]: the control-law catalog on the wire.
//! - [`server`]: the WebSocket service, loopback only.
//!
//! Hardware is reached only through `hertzforge-station`, the controller's one
//! construction point; a run's `source` is `HARDWARE` only on physical NI devices,
//! as the driver states them (`tests/forbidden_deps.rs` holds the dependency line).
#![forbid(unsafe_code)]

pub mod codec;
pub mod explorer;
pub mod laws;
pub mod mimo;
pub mod mixed;
pub mod profiles;
pub mod protocol;
pub mod pulse;
pub mod random;
pub mod recorder;
pub mod server;
pub mod session;
pub mod shock;
pub mod sim;
pub mod sine;
pub mod srs;
pub mod station;
pub mod table;
pub mod twr;
pub mod views;

/// Default bind address.
pub const DEFAULT_HOST: &str = "127.0.0.1";
/// Default port.
pub const DEFAULT_PORT: u16 = 8792;
