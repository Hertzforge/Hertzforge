//! The transient-run rails: what a run that fires one pulse at a time needs from the
//! control service, whatever the pulse is (classical shock now; SRS shares them).
//!
//! - [`rails`]: the published view and the run log, the job queue the run thread
//!   takes requests from, the signals Stop, Abort and an absent controller raise
//!   without the run thread (the cancel flag a fire in flight reads at every block,
//!   the hold that keeps the next pulse from starting), the fire's progress written
//!   from the per-block observer, a wait that keeps answering requests, and
//!   `catch_unwind` around each step so a panic ends in the emergency stop.
//! - [`ring`]: the bounded store of the most recent pulses' figures.

pub mod rails;
pub mod ring;
