//! The math of the shock control laws: each law's declaration (name, version,
//! parameters, the values it reports) and its arithmetic, with nothing around
//! it.
//!
//! - [`classical`]: `fd_correct`, the built-in classical shock law. It is the
//!   frequency-domain correction time waveform replication uses, applied to the
//!   compensated pulse: a seed from the regularized inverse of the identified
//!   plant times the reference, then a correction of the drive spectrum by a
//!   fraction of the inverse times the error after each pulse, on the bins the
//!   core trusts, with the growth of each bin limited per pulse.
//!
//! The law files wrap this code in the law interface and the controller's
//! built-in native laws wrap it in the controller's own law traits, so the two
//! are one implementation. The controller applies its own protections to the
//! drive either returns.
//!
//! # Conventions
//!
//! Spectra follow the `shock-classical` world (`wit/control-law/shock-classical.wit`):
//! the one-sided transform of a record under the negative-exponent transform,
//! `record_len / 2 + 1` bins. `hertzforge_dsp`'s FFT is positive-exponent; the
//! spectra this crate forms are its conjugates, so a plant the core hands a law
//! and the spectra a law forms itself agree.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod classical;
pub mod spectrum;

/// A complex value as `(re, im)`, the workspace's own representation.
pub use hertzforge_dsp::fft::Cplx;

/// The author named in every built-in shock law's manifest.
pub const AUTHOR: &str = "Hertzforge contributors";

/// A real-number parameter a law declares.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumberParam {
    /// Stable key.
    pub key: &'static str,
    /// Name shown on the form.
    pub label: &'static str,
    /// Unit shown next to the box. May be empty.
    pub unit: &'static str,
    /// Help text for the form.
    pub help: &'static str,
    /// Smallest accepted value.
    pub min: f64,
    /// Largest accepted value.
    pub max: f64,
    /// How far one step of the box moves the value (a hint for the form).
    pub step: f64,
    /// The value used when none is given.
    pub default: f64,
}

/// A named number a law reports on each call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticDecl {
    /// Stable key.
    pub key: &'static str,
    /// Name shown on a plot or table.
    pub label: &'static str,
    /// Unit.
    pub unit: &'static str,
    /// Help text.
    pub help: &'static str,
}
