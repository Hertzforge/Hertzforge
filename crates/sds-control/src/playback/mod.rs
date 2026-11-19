//! Playback through the protocol (schema/PROTOCOL.md, "Playback"): a drive file the
//! operator made, played open loop on every drive it maps to, with every input and
//! every drive recorded and no analysis. The configuration and the file's check, the
//! run around the core's run host, its state and live frames, and its record.

pub mod config;
pub mod records;
pub mod run;
