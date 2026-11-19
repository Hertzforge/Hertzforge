//! The plays table: one row per play after the identification, what it played
//! and how the core judged it.

use serde::{Deserialize, Serialize};

use hertzforge_shock_common::law_guard::{LawFault, LawValue};

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
    /// The control law's call behind the drive this play played: the seed
    /// before the first, then the call after the play before it. `None`
    /// without a control law, and for a play with no call behind it (a gated
    /// level carried forward, or adaptation frozen).
    pub law: Option<PlayLawCall>,
}

/// One control-law call, as the row of the play its drive played on shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayLawCall {
    /// The call's drive is the one this play played. False after a miss: the
    /// drive in force replayed.
    pub used: bool,
    /// Why the call was not used.
    pub miss: Option<LawFault>,
    /// How long the call took, milliseconds.
    pub call_ms: f64,
    /// The rise of the drive's peak over the drive before it, dB (`None` at the
    /// seed, and when the call gave no drive to compare).
    pub peak_rise_db: Option<f64>,
    /// The largest rise of any drive bin over the drive before it, dB.
    pub bin_rise_db: Option<f64>,
    /// The drive's predicted response peak over the level target's.
    pub predicted_peak_ratio: Option<f64>,
    /// The values the law reported.
    pub values: Vec<LawValue>,
}
