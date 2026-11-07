//! `fd_correct`: the built-in classical shock law.
//!
//! - **Seed.** `D0 = G R` on the trusted bins, zero elsewhere, scaled down by
//!   `initial_backoff_db`. `R` is the spectrum of the full-level reference and
//!   `G` the core's regularized inverse of the identified plant.
//! - **Each pulse.** `D += beta G (R - Y)` on the trusted bins, `Y` the spectrum
//!   of the last response referred to full level. A bin's magnitude may grow by
//!   at most `max_db_per_pulse` in one pulse; it may always shrink.
//! - **The drive** is the inverse transform of `D`. The law keeps its own `D`,
//!   as time waveform replication does, rather than re-deriving it from the
//!   drive the core played.
//!
//! The seed and the correction are the same arithmetic as time waveform
//! replication's initial drive and correction, bin for bin; the cross-check is
//! `tests/twr_formula.rs`.
//!
//! This module is the law's declaration and its arithmetic and nothing else.
//! The controller's native law and the law file `laws/shock-fdc` both wrap it.

use crate::{Cplx, DiagnosticDecl, NumberParam};

/// The law's catalog id.
pub const ID: &str = "fd_correct";
/// The law's name in its manifest.
pub const NAME: &str = "frequency-domain correction";
/// The law's version in its manifest.
pub const VERSION: &str = "0.1.0";
/// What the law does, for its manifest.
pub const SUMMARY: &str = "Starts from the inverse of the identified plant applied to the \
    reference pulse, then after each pulse corrects the drive spectrum by a fraction of the \
    inverse times the error, on the bins the controller trusts. Each bin's growth is limited \
    per pulse.";

/// The parameters, in form order: `beta`, `max_db_per_pulse`,
/// `initial_backoff_db`.
pub const KNOBS: &[NumberParam] = &[
    NumberParam {
        key: "beta",
        label: "Correction gain",
        unit: "",
        help: "The fraction of each pulse's error corrected before the next pulse. 1 corrects \
               the whole error at once; smaller values settle more gently when the plant \
               estimate is uncertain.",
        min: 0.05,
        max: 1.0,
        step: 0.05,
        default: 0.5,
    },
    NumberParam {
        key: "max_db_per_pulse",
        label: "Largest rise per pulse",
        unit: "dB",
        help: "The most any one frequency of the drive may grow from one pulse to the next. \
               A frequency may always fall.",
        min: 1.0,
        max: 9.0,
        step: 0.5,
        default: 6.0,
    },
    NumberParam {
        key: "initial_backoff_db",
        label: "First pulse backoff",
        unit: "dB",
        help: "How far under the predicted drive the first pulse starts. The correction \
               brings the level up over the next pulses.",
        min: 0.0,
        max: 12.0,
        step: 1.0,
        default: 0.0,
    },
];

/// The values the law reports on each call.
pub const DIAGNOSTICS: &[DiagnosticDecl] = &[
    DiagnosticDecl {
        key: "corrected_bins",
        label: "Corrected bins",
        unit: "bins",
        help: "Bins whose drive the law moved on this call.",
    },
    DiagnosticDecl {
        key: "clamped_bins",
        label: "Bins at the rise limit",
        unit: "bins",
        help: "Bins whose rise the law held to its largest rise per pulse on this call.",
    },
    DiagnosticDecl {
        key: "drive_change_db",
        label: "Drive change",
        unit: "dB",
        help: "The drive's peak against the last drive's, in dB (0 at the seed).",
    },
];

/// The parameter values a law is built with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FdCorrectParams {
    /// Fraction of each pulse's error corrected per pulse.
    pub beta: f64,
    /// The most a bin's magnitude may grow in one pulse, dB.
    pub max_db_per_pulse: f64,
    /// How far under the predicted drive the seed starts, dB.
    pub initial_backoff_db: f64,
}

impl Default for FdCorrectParams {
    /// The declared defaults.
    fn default() -> Self {
        FdCorrectParams {
            beta: KNOBS[0].default,
            max_db_per_pulse: KNOBS[1].default,
            initial_backoff_db: KNOBS[2].default,
        }
    }
}

impl FdCorrectParams {
    /// Set the parameter `key` to `value`, checked against its declaration.
    /// Leaves the values unchanged on an unknown key or an out-of-range value.
    pub fn set(&mut self, key: &str, value: f64) -> Result<(), String> {
        let Some(p) = KNOBS.iter().find(|p| p.key == key) else {
            return Err(format!("`{key}` is not a parameter of this law"));
        };
        if !value.is_finite() || value < p.min || value > p.max {
            return Err(format!(
                "{key} is {value}; it runs from {} to {}",
                p.min, p.max
            ));
        }
        match key {
            "beta" => self.beta = value,
            "max_db_per_pulse" => self.max_db_per_pulse = value,
            _ => self.initial_backoff_db = value,
        }
        Ok(())
    }
}

/// What one call did besides producing the drive: the values behind
/// [`DIAGNOSTICS`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StepReport {
    /// Bins whose drive moved.
    pub corrected_bins: usize,
    /// Bins whose rise was held to `max_db_per_pulse`.
    pub clamped_bins: usize,
    /// The drive's peak against the last drive's, dB (0 at the seed).
    pub drive_change_db: f64,
}

impl StepReport {
    /// The report as `(key, value)` pairs, in [`DIAGNOSTICS`] order.
    pub fn values(&self) -> [(&'static str, f64); 3] {
        [
            (DIAGNOSTICS[0].key, self.corrected_bins as f64),
            (DIAGNOSTICS[1].key, self.clamped_bins as f64),
            (DIAGNOSTICS[2].key, self.drive_change_db),
        ]
    }
}

/// One call's result: the next full-level drive record, volts, and the report.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// `record_len` samples.
    pub drive: Vec<f64>,
    /// What the call did.
    pub report: StepReport,
}

/// The law's state for one run.
// The seed and the correction read the reference, the inverse, the trusted
// bins and the drive spectrum; until they are written nothing does.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FdCorrect {
    params: FdCorrectParams,
    record_len: usize,
    reference: Vec<f64>,
    inverse: Vec<Cplx>,
    trusted: Vec<bool>,
    /// The law's own drive spectrum `D`, kept between calls.
    drive_spectrum: Vec<Cplx>,
}

impl FdCorrect {
    /// Build the law for a run: the full-level `reference` record
    /// (`record_len` samples), and the core's regularized `inverse` and
    /// `trusted` bins on the grid (`record_len / 2 + 1` each).
    pub fn new(
        params: FdCorrectParams,
        record_len: usize,
        reference: &[f64],
        inverse: &[Cplx],
        trusted: &[bool],
    ) -> Result<Self, String> {
        let bins = record_len / 2 + 1;
        if record_len < 2 {
            return Err(format!("a record of {record_len} samples is too short"));
        }
        if reference.len() != record_len {
            return Err(format!(
                "the reference has {} samples; the record has {record_len}",
                reference.len()
            ));
        }
        if inverse.len() != bins || trusted.len() != bins {
            return Err(format!(
                "the plant has {} inverse bins and {} trusted flags; the record has {bins} bins",
                inverse.len(),
                trusted.len()
            ));
        }
        Ok(FdCorrect {
            params,
            record_len,
            reference: reference.to_vec(),
            inverse: inverse.to_vec(),
            trusted: trusted.to_vec(),
            drive_spectrum: Vec::new(),
        })
    }

    /// The parameters the law was built with.
    pub fn params(&self) -> &FdCorrectParams {
        &self.params
    }

    /// The record length the law was built for.
    pub fn record_len(&self) -> usize {
        self.record_len
    }

    /// The first drive, before any pulse has fired.
    pub fn seed(&mut self) -> Result<Step, String> {
        Err("the frequency-domain correction law is not built yet".into())
    }

    /// The next drive, from the last pulse's `response` referred to full level
    /// (`record_len` samples).
    pub fn iterate(&mut self, response: &[f64]) -> Result<Step, String> {
        if response.len() != self.record_len {
            return Err(format!(
                "the response has {} samples; the record has {}",
                response.len(),
                self.record_len
            ));
        }
        Err("the frequency-domain correction law is not built yet".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_declared_ones_and_inside_their_ranges() {
        let p = FdCorrectParams::default();
        assert_eq!(
            (p.beta, p.max_db_per_pulse, p.initial_backoff_db),
            (0.5, 6.0, 0.0)
        );
        for k in KNOBS {
            assert!(k.min <= k.default && k.default <= k.max, "{}", k.key);
        }
    }

    #[test]
    fn set_checks_the_key_and_the_range() {
        let mut p = FdCorrectParams::default();
        p.set("beta", 0.25).unwrap();
        assert_eq!(p.beta, 0.25);
        assert!(p.set("beta", 0.0).is_err());
        assert!(p.set("gain", 0.5).is_err());
        assert!(p.set("max_db_per_pulse", f64::NAN).is_err());
        assert_eq!(p.beta, 0.25, "a refused value changes nothing");
    }

    #[test]
    fn new_refuses_mismatched_lengths_and_the_unbuilt_calls_say_so() {
        let r = vec![0.0; 8];
        let g = vec![(1.0, 0.0); 5];
        let t = vec![true; 5];
        assert!(FdCorrect::new(FdCorrectParams::default(), 8, &r[..7], &g, &t).is_err());
        assert!(FdCorrect::new(FdCorrectParams::default(), 8, &r, &g[..4], &t).is_err());
        let mut law = FdCorrect::new(FdCorrectParams::default(), 8, &r, &g, &t).unwrap();
        assert!(law.seed().unwrap_err().contains("not built"));
        assert!(law.iterate(&r[..3]).unwrap_err().contains("samples"));
    }

    #[test]
    fn the_report_values_follow_the_declarations() {
        let v = StepReport {
            corrected_bins: 3,
            clamped_bins: 1,
            drive_change_db: -0.5,
        }
        .values();
        let keys: Vec<&str> = v.iter().map(|(k, _)| *k).collect();
        let declared: Vec<&str> = DIAGNOSTICS.iter().map(|d| d.key).collect();
        assert_eq!(keys, declared);
        assert_eq!(v[0].1, 3.0);
    }
}
