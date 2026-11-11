//! Single Output Mixed Mode: sine tones and narrow bands of random on a random
//! floor, each part dwelling, sweeping or following another at a ratio, played
//! together on one drive and controlled together.
//!
//! - [`parts`]: the parts list, its schedules and the checks on a configuration.
//!
//! The control method composes published building blocks (tone estimation in
//! noise, Welch estimation, inverse-plant equalization) and reuses the Random
//! and Sine cores' laws under their own guards.

#![forbid(unsafe_code)]

pub mod parts;

pub use parts::{
    sweep_span, BandWidth, ConfigError, Direction, FloorPart, LawPick, MixedConfig,
    MixedIdentification, MixedLawChoice, MixedLawGuards, MixedLimits, Motion, NarrowbandPart, Part,
    PartBody, PartSchedule, PartSweepTarget, PartWindow, SweepLaw, ToneAmplitude, TonePart,
    DEFAULT_EXCLUSION_HZ, DEFAULT_FLOOR_LAW_ID, DEFAULT_TONE_LAW_ID, DEFAULT_TRACKING_HZ,
    MAX_BANDS, MAX_TONES, MIN_TONE_SPACING_WIDTHS, RESERVED_PART_KINDS,
};
