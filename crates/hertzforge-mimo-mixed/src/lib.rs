//! MIMO Mixed Mode: sine tones and narrowband random on a broadband random
//! floor, on several shakers controlled together at several control points.
//!
//! This build is the contracts and the mathematics, pure (no hardware, no
//! service):
//!
//! - [`config`]: [`MimoMixedConfig`], its parts and its validation, refusing
//!   by name.
//! - [`estimate`]: the matrix floor estimate with per-line pause and resume.
//! - [`exclusion`]: which lines each analysis frame leaves out.
//! - [`fill`]: what a paused line shows the floor law (`no_error`,
//!   `model_fill`).
//! - [`band`]: band statements, classification, folding, the moving-frame
//!   estimate and the band laws (`scalar_servo`, `per_control`; `auto`
//!   resolves).
//! - [`tone`]: the multi-drive tone bank, its trackers and the residual.
//! - [`budget`]: the shared drive budget, the ceiling rules (`common_scale`,
//!   `in_turn`) and the setup forecast.
//! - [`predict`]: the setup prediction per drive and control.
//! - [`mat`]: small dense complex matrices for the above.
//!
//! Conventions are the MIMO worlds': `G = E[X X^H]` (entry `(i, j)` is
//! `E[X_i conj(X_j)]`), one-sided densities, `C = H D` for tones with drive
//! `Re{ D exp(j phi) }`, amplitudes peak, everything at full level unless
//! stated.

#![forbid(unsafe_code)]

pub mod band;
pub mod budget;
pub mod config;
pub mod estimate;
pub mod exclusion;
pub mod fill;
pub mod mat;
pub mod predict;
#[cfg(feature = "service")]
pub mod service;
pub mod tone;

pub use band::{
    band_law, classify, fold_into_reference, increment, resolve_band_law, BandClass, BandLaw,
    BandLawError, BandLawParams, BandLoopInputs, BandLoopOutput, BandResolution, BandSetup,
    BandStatement, DriveDeclared, MovingFrameEstimate, PerControlServo, ScalarServo,
};
pub use budget::{
    ceiling_rule, forecast, headroom, BudgetForecast, CeilingRule, CommonScale, InTurn,
};
pub use config::{
    MimoDrive, MimoFloorPart, MimoMixedConfig, MimoMixedConfigError, MimoMixedLawChoice,
    MimoNarrowbandPart, MimoPart, MimoPartBody, MimoTonePart, PerControl, BAND_LAW_IDS,
    CEILING_RULE_IDS, DEFAULT_BAND_LAW_ID, DEFAULT_CEILING_RULE_ID, DEFAULT_FILL_RULE_ID,
    DEFAULT_FLOOR_LAW_ID, DEFAULT_TONE_LAW_ID, FILL_RULE_IDS, MAX_BANDS, MAX_CONTROLS, MAX_DRIVES,
    MAX_LIMITS, MAX_TONES, TEST_TYPE_ID,
};
pub use estimate::{FrameSpectra, MatrixFloorEstimator};
pub use exclusion::{frame_mask, ToneSpan};
pub use fill::{fill_rule, FillInput, FillReport, FillRule, ModelFill, NoError};
pub use predict::{predict, ControlPrediction, DrivePrediction, MimoMixedPrediction};
pub use tone::{DriveRamp, ToneBank, ToneLock};
