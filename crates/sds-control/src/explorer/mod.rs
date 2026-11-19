//! Explorer through the protocol (schema/PROTOCOL.md, "Explorer"): one sine tone the
//! operator steers by hand, with no control loop. The configuration and its checks, the
//! run around the core's run host, the state and live frames, the subscriptions and the
//! run's records.

pub mod config;
pub mod records;
pub mod run;
pub mod subscribe;
pub mod view;
