//! Single Output Mixed Mode: sine tones and narrow bands of random on a random
//! floor, each part dwelling, sweeping or following another at a ratio, played
//! together on one drive and controlled together.
//!
//! - [`parts`]: the parts list, its schedules and the checks on a configuration.
//! - [`schedule`]: the clock that moves every part, held and resumed together.
//! - [`service`] (feature `service`, on by default): [`MixedService`], the run
//!   lifecycle a control service drives, and its status, display and report.
//!
//! The control method composes published building blocks (tone estimation in
//! noise, Welch estimation, inverse-plant equalization) and reuses the Random
//! and Sine cores' laws under their own guards.

#![forbid(unsafe_code)]

pub mod band;
pub mod floor;
pub mod parts;
pub mod predict;
pub mod schedule;
#[cfg(feature = "service")]
pub mod service;
#[cfg(test)]
mod testkit;
pub mod tone;
#[cfg(test)]
mod verify;

pub use parts::{
    sweep_span, BandWidth, ConfigError, Direction, FloorPart, LawPick, MixedConfig,
    MixedIdentification, MixedLawChoice, MixedLawGuards, MixedLimits, Motion, NarrowbandPart, Part,
    PartBody, PartSchedule, PartSweepTarget, PartWindow, SweepLaw, ToneAmplitude, TonePart,
    DEFAULT_EXCLUSION_HZ, DEFAULT_FLOOR_LAW_ID, DEFAULT_FLOOR_SETTLE_S, DEFAULT_PART_RAMP_S,
    DEFAULT_TONE_LAW_ID, DEFAULT_TRACKING_HZ, MAX_BANDS, MAX_TONES, MIN_TONE_SPACING_WIDTHS,
    RESERVED_PART_KINDS,
};
pub use predict::{predict, sine_gauss_exceedance, MixedPrediction, PartMeanSquare, RatingCheck};
pub use schedule::{PartClock, PartNow};
#[cfg(feature = "service")]
pub use service::display::{
    BandHistory, LineGrid, MixedDisplay, MixedDisplayChannel, MixedFrf, ToneHistory,
};
#[cfg(feature = "service")]
pub use service::status::{
    FloorSummary, MixedEvent, MixedEventKind, MixedLawStatus, MixedLog, MixedOutcome, MixedPhase,
    MixedState, MixedStatus, MixedTotals, MixedTrip, MixedTripKind, PartKind, PartRow, RoleTally,
};
#[cfg(feature = "service")]
pub use service::{
    LawRole, MixedAdjustRefusal, MixedError, MixedLawFactories, MixedReport, MixedService,
};
