//! MIMO Sine laws: the `sine-mimo` world (several drives, several control
//! channels, one tone at a time, one complex amplitude per drive each control
//! update).
//!
//! A MIMO Sine law is called once per control update on the controller's
//! real-time thread: `engage` for the first drive after identification, `tick`
//! for every update after it. It returns one complex amplitude per drive, in
//! volts peak at the scheduled level, its own fault judgement, up to eight
//! values its manifest declares, and optionally a request to hold or slow the
//! sweep and a notched reference.
//!
//! This module holds the world's types as the interface generates them. The
//! law trait and its export macro are not in this version.

/// The world's records, as the interface generates them: the setup, the
/// inputs and outputs of one update, the request, the hold, the shapes and
/// the test types, and the manifest.
pub use crate::__sine_mimo_bindings::hertzforge::control_law::mimo_sine_types::{
    Complex, Hold, Identification, MatrixStack, ProportionalTracking, Request, SineMimoInputs,
    SineMimoManifest, SineMimoOutputs, SineMimoSetup, SineShape, TestType, Tolerance, Tracking,
};

/// The most values a MIMO Sine law may report on one call.
pub const MAX_VALUES: usize = 8;
