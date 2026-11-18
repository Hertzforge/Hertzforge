//! The mathematics of the multiple-drive sine control laws: one tone played on
//! several drives, each with its own amplitude and phase, tracked at several
//! control points, with nothing around it.
//!
//! The controller owns the tone, the schedule, every guard and every abort;
//! each update it tells a law the frequency, the sweep rate, the level, the
//! reference, the tracked controls and the drive in force, and the law
//! returns one complex amplitude per drive. The laws here are plain structs
//! with an `engage` (the first drive) and a `tick` (every drive after it):
//!
//! - [`matrix`]: the linear law and the default. Correction through the
//!   identification's regularized inverse at the loop-speed rule's gain,
//!   carried along the model in a sweep, rebased on the drive in force, with
//!   the drive the controls cannot see reset where there are spare drives.
//! - [`paired`]: each control paired with one drive, that drive's amplitude
//!   following the ratio of wanted to measured amplitude. No model of the
//!   coupling; the contributor's template.
//! - [`steepest`], [`secant`], [`ratio`]: the three other published laws,
//!   as the research testbed runs them: steepest descent, the secant law with
//!   a complex gain, and the amplitude-only ratio law per control.
//! - [`speed`]: the loop-speed rule, the gain per update.
//! - [`modal`]: the model the carry reads (modes exact, remainder smoothed).
//! - [`setup`]: the setup, interpolation at a frequency, the identification's
//!   group delay.
//! - [`readings`]: the opposition ratio, the largest share one shaker carries,
//!   and the sweep-rate warning.
//!
//! No law module uses a clock, thread, file or random number, so a law file
//! built for WebAssembly and the controller's native laws run the same code.
//! The `reference-structure` feature adds an exact linear modal structure with
//! the tracking filter's lag, for tests; no law uses it.
//!
//! # Conventions
//!
//! - A drive's amplitude `D_i` plays `Re{D_i exp(j phi(t))}`, volts peak; a
//!   channel's tracked amplitude `C_c` reads `Re{C_c exp(j phi(t))}`, the
//!   phase of the response minus the tone's, so `C = H D` for a linear
//!   structure in steady state.
//! - Matrices are row-major, controls by drives (an inverse drives by controls).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod law;
pub mod linalg;
pub mod matrix;
pub mod modal;
pub mod paired;
pub mod ratio;
pub mod readings;
pub mod secant;
pub mod setup;
pub mod speed;
pub mod steepest;

#[cfg(feature = "reference-structure")]
pub mod reference_structure;

pub use hertzforge_sdm::{MatrixStack, C64};
pub use law::{Hold, Inputs, Outputs, Request};
pub use setup::{
    DelayEstimate, Identification, LoopDelay, Setup, Shape, TrackingRule,
    DEFAULT_TRACKER_DELAY_FACTOR,
};

/// The test types a law accepts. The controller refuses a setup that asks a
/// law for a type it does not accept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TestTypes {
    /// Sweeps (logarithmic or linear).
    pub sweep: bool,
    /// Dwells at a frequency.
    pub dwell: bool,
    /// Stepped sine.
    pub stepped: bool,
}
