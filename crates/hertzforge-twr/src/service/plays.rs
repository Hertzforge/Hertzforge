//! The plays table: one row per play after the identification, what it played
//! and how the core judged it.

use serde::{Deserialize, Serialize};

use crate::control::PlayKind;
use crate::metrics::ErrorMetrics;

/// One play after the identification.
///
/// A play that was played and measured has a row, the one a response check
/// aborted on included. A play the core refused before it reached the station
/// has none, and neither has a play a stop or an abort cancelled (it was never
/// judged); the frames a cancelled play clocked still count in station time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayRow {
    /// The row's place in the table, from 0.
    pub index: u64,
    /// A correction play on the ladder, or a scheduled playback play.
    pub kind: PlayKind,
    /// The commanded level it played at, dB.
    pub level_db: f64,
    /// The table level it played at, or `None` when a manual trim had moved the
    /// level off the table's levels.
    pub rung: Option<usize>,
    /// The correction play's count at its level, from 1. `None` for a playback
    /// play.
    pub iteration: Option<u32>,
    /// The core's error metrics for the play.
    pub metrics: ErrorMetrics,
    /// The play met the convergence gate at its level. A playback play is
    /// judged against the same gate; a play a response check aborted on is not
    /// gated.
    pub gated: bool,
    /// Drive samples the per-sample clip held.
    pub clipped_samples: usize,
    /// Bins the correction's growth clamp held back after this play.
    pub clamped_bins: usize,
    /// The drive's peak as played, volts.
    pub drive_vpk: f64,
    /// The response's peak as measured, control units.
    pub response_peak: f64,
    /// The play's fitted lag, frames. `None` until a play's own lag is fitted.
    pub lag_frames: Option<f64>,
    /// The fitted lag's drift from the identification's, frames. `None` until a
    /// play's own lag is fitted.
    pub drift_frames: Option<f64>,
    /// The station frame the play started at: the frames the station clocked
    /// over every play and pad before it.
    pub start_frame: u64,
    /// The frames the station clocked for the play: the record and its rest
    /// pad.
    pub frames: u64,
    /// `start_frame` in seconds at the rate the station runs at.
    pub at_s: f64,
}
