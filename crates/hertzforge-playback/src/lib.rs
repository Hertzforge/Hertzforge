//! `hertzforge-playback` - Playback, the open-loop multi-drive player.
//!
//! The operator makes a drive: volts at each drive output, one channel per drive, any
//! length. The analysis side converts it from whatever format it was made in into the
//! one native drive file this crate reads ([`file`]). Before a run arms, the control
//! service checks the file ([`check`]): whole, finite, its SHA-256 as its sidecar and
//! the setup state, and per channel starting and ending at zero acceleration,
//! velocity and displacement with the drive taken as an acceleration; then the
//! setup with it ([`plan`]): the map to distinct drive rows, the rate, the peak at the
//! level within the ceiling. A run arms only with a [`plan::CheckedDrive`].
//!
//! The run ([`host`]) reads the file ahead on its own thread into a bounded ring
//! ([`source`]), opens the station at the file's rate, and ticks the engine with the
//! process step ([`step`]): pre-roll at zero, the file at the level, post-roll at zero.
//! Nothing closes a loop. The engine's watchdog reads the channel table's abort limits
//! on every sample; a stop ramps the streamed drive down; an abort, an underrun, a
//! ceiling trip, the reader's failure or a panic zeroes the drive at once and ends in
//! the emergency stop.
//!
//! This crate names no hardware backend: it is handed a finished station backend.
#![forbid(unsafe_code)]

pub mod check;
pub mod file;
pub mod host;
pub mod plan;
pub mod source;
pub mod step;

pub use check::{check_file, ChannelCheck, FileCheck, PureIntegrator};
pub use file::{DriveFile, FileError};
pub use host::{HostError, HostOptions, HostStatus, Outcome, Pacing, PlaybackHost};
pub use plan::{CheckedDrive, DriveRow, PlaySetup, StationOffer};
pub use step::Phase;
