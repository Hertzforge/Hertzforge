//! `twr_correct`: the built-in time waveform replication law.
//!
//! The same frequency-domain correction as [`crate::classical`], applied to a
//! whole replication record between plays:
//!
//! - **Seed.** `D0 = G R` on the trusted bins, zero elsewhere, scaled down by
//!   `initial_backoff_db`. `R` is the spectrum of the full-level reference
//!   record and `G` the core's regularized inverse of the identified plant.
//! - **Each correction.** `D += beta G (R - Y)` on the trusted bins, `Y` the
//!   spectrum of the last response referred to full level. A bin's magnitude
//!   may grow by at most `max_db_per_iter` in one call; it may always shrink.
//! - **A clipped play.** When the core clipped the last drive, the law first
//!   takes the drive as played as its own (`D` becomes that record's
//!   spectrum), then corrects it: the correction measures the drive that
//!   actually played, not the one it asked for.
//!
//! Everything is at full level: the core scales the drive to the level it
//! commands and refers the response back by the scale it commanded.
//!
//! This module is the law's declaration and its arithmetic and nothing else.
//! The controller's native law and the law file `laws/twr-correct` both wrap
//! it, so the two are one implementation.

use crate::classical::{FdCorrect, FdCorrectParams, Step};
use crate::{Cplx, DiagnosticDecl, NumberParam};

/// The law's catalog id.
pub const ID: &str = "twr_correct";
/// The law's name in its manifest.
pub const NAME: &str = "frequency-domain correction";
/// The law's version in its manifest.
pub const VERSION: &str = "0.1.0";
/// What the law does, for its manifest.
pub const SUMMARY: &str = "Starts from the inverse of the identified plant applied to the \
    reference record, then after each correction play corrects the drive spectrum by a fraction \
    of the inverse times the error, on the bins the controller trusts. Each bin's growth is \
    limited per correction. When the controller clipped a play's drive, the correction starts \
    from the drive as played.";

/// The parameters, in form order: `beta`, `max_db_per_iter`,
/// `initial_backoff_db`.
pub const KNOBS: &[NumberParam] = &[
    NumberParam {
        key: "beta",
        label: "Correction gain",
        unit: "",
        help: "The fraction of each play's error corrected before the next play. 1 corrects \
               the whole error at once; smaller values settle more gently when the plant \
               estimate is uncertain.",
        min: 0.05,
        max: 1.0,
        step: 0.05,
        default: 0.5,
    },
    NumberParam {
        key: "max_db_per_iter",
        label: "Largest rise per correction",
        unit: "dB",
        help: "The most any one frequency of the drive may grow from one correction to the \
               next. A frequency may always fall.",
        min: 1.0,
        max: 9.0,
        step: 0.5,
        default: 6.0,
    },
    NumberParam {
        key: "initial_backoff_db",
        label: "First play backoff",
        unit: "dB",
        help: "How far under the predicted drive the first play starts. The correction brings \
               the level up over the next plays.",
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
        help: "Bins whose rise the law held to its largest rise per correction on this call.",
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
pub struct TwrCorrectParams {
    /// Fraction of each play's error corrected per call.
    pub beta: f64,
    /// The most a bin's magnitude may grow in one call, dB.
    pub max_db_per_iter: f64,
    /// How far under the predicted drive the seed starts, dB.
    pub initial_backoff_db: f64,
}

impl Default for TwrCorrectParams {
    /// The declared defaults.
    fn default() -> Self {
        TwrCorrectParams {
            beta: KNOBS[0].default,
            max_db_per_iter: KNOBS[1].default,
            initial_backoff_db: KNOBS[2].default,
        }
    }
}

impl TwrCorrectParams {
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
            "max_db_per_iter" => self.max_db_per_iter = value,
            _ => self.initial_backoff_db = value,
        }
        Ok(())
    }

    /// Check every value against its declaration.
    pub fn check(&self) -> Result<(), String> {
        for (key, v) in [
            ("beta", self.beta),
            ("max_db_per_iter", self.max_db_per_iter),
            ("initial_backoff_db", self.initial_backoff_db),
        ] {
            let p = KNOBS.iter().find(|p| p.key == key).expect("declared");
            if !v.is_finite() || v < p.min || v > p.max {
                return Err(format!("{key} is {v}; it runs from {} to {}", p.min, p.max));
            }
        }
        Ok(())
    }

    fn classical(&self) -> FdCorrectParams {
        FdCorrectParams {
            beta: self.beta,
            max_db_per_pulse: self.max_db_per_iter,
            initial_backoff_db: self.initial_backoff_db,
        }
    }
}

/// The law's state for one run.
#[derive(Debug, Clone)]
pub struct TwrCorrect {
    params: TwrCorrectParams,
    inner: FdCorrect,
}

impl TwrCorrect {
    /// Build the law for a run: the full-level `reference` record
    /// (`record_len` samples, a power of two), and the core's regularized
    /// `inverse` and `trusted` bins on the grid (`record_len / 2 + 1` each).
    pub fn new(
        params: TwrCorrectParams,
        record_len: usize,
        reference: &[f64],
        inverse: &[Cplx],
        trusted: &[bool],
    ) -> Result<Self, String> {
        params.check()?;
        let inner = FdCorrect::new(params.classical(), record_len, reference, inverse, trusted)?;
        Ok(TwrCorrect { params, inner })
    }

    /// The parameters the law was built with.
    pub fn params(&self) -> &TwrCorrectParams {
        &self.params
    }

    /// The record length the law was built for.
    pub fn record_len(&self) -> usize {
        self.inner.record_len()
    }

    /// The law's drive spectrum `D` (empty before the seed).
    pub fn drive_spectrum(&self) -> &[Cplx] {
        self.inner.drive_spectrum()
    }

    /// The first drive, before any play: `D0 = G R` on the trusted bins, zero
    /// elsewhere, then scaled down by `initial_backoff_db`.
    pub fn seed(&mut self) -> Result<Step, String> {
        self.inner.seed()
    }

    /// The next drive, after a judged play. `response` is the play's response
    /// referred to full level (`record_len` samples). `clipped` is the drive
    /// the play played, referred to full level, when the core clipped it: the
    /// law then corrects from that drive rather than the one it asked for.
    pub fn iterate(&mut self, response: &[f64], clipped: Option<&[f64]>) -> Result<Step, String> {
        if let Some(played) = clipped {
            self.inner.adopt(played)?;
        }
        self.inner.iterate(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(x: &[f64]) -> f64 {
        x.iter().fold(0.0f64, |m, v| m.max(v.abs()))
    }

    #[test]
    fn the_defaults_are_the_declared_ones_and_inside_their_ranges() {
        let p = TwrCorrectParams::default();
        assert_eq!(
            (p.beta, p.max_db_per_iter, p.initial_backoff_db),
            (0.5, 6.0, 0.0)
        );
        p.check().unwrap();
        for k in KNOBS {
            assert!(k.min <= k.default && k.default <= k.max, "{}", k.key);
        }
        // The ranges are the classical law's, which this law is built over.
        for (a, b) in KNOBS.iter().zip(crate::classical::KNOBS) {
            assert_eq!((a.min, a.max, a.default), (b.min, b.max, b.default));
        }
        let keys: Vec<&str> = DIAGNOSTICS.iter().map(|d| d.key).collect();
        let classical: Vec<&str> = crate::classical::DIAGNOSTICS
            .iter()
            .map(|d| d.key)
            .collect();
        assert_eq!(keys, classical, "the step report's values are these keys");
    }

    #[test]
    fn set_and_check_hold_the_declared_ranges() {
        let mut p = TwrCorrectParams::default();
        p.set("max_db_per_iter", 3.0).unwrap();
        assert_eq!(p.max_db_per_iter, 3.0);
        assert!(p.set("max_db_per_pulse", 3.0).is_err(), "the classical key");
        assert!(p.set("beta", 1.5).is_err());
        assert!(p.set("initial_backoff_db", f64::NAN).is_err());
        let bad = TwrCorrectParams {
            beta: 0.01,
            ..TwrCorrectParams::default()
        };
        assert!(bad.check().unwrap_err().contains("beta"));
        let r = vec![0.0; 8];
        let g = vec![(1.0, 0.0); 5];
        let t = vec![true; 5];
        assert!(TwrCorrect::new(bad, 8, &r, &g, &t).is_err());
    }

    /// On a plant measuring 0.8 of the drive, a play the core clipped to half
    /// is corrected from the half: the next drive is the half plus beta times
    /// the inverse of the remaining error, not the unclipped drive plus it.
    #[test]
    fn a_clipped_play_is_corrected_from_the_drive_as_played() {
        let n = 64;
        let reference: Vec<f64> = (0..n)
            .map(|i| (std::f64::consts::TAU * 4.0 * i as f64 / n as f64).sin())
            .collect();
        let bins = n / 2 + 1;
        let g = vec![(1.25, 0.0); bins];
        let t = vec![true; bins];
        let mut law = TwrCorrect::new(TwrCorrectParams::default(), n, &reference, &g, &t).unwrap();
        let seed = law.seed().unwrap();
        // The seed is 1.25 R; the core played half of it.
        let half: Vec<f64> = seed.drive.iter().map(|v| 0.5 * v).collect();
        let response: Vec<f64> = half.iter().map(|v| 0.8 * v).collect();
        let mut clipped_law = law.clone();
        let from_half = clipped_law.iterate(&response, Some(&half)).unwrap();
        let unclipped = law.iterate(&response, None).unwrap();
        // From the half: 0.625 R + 0.5 * 1.25 * (R - 0.5 R) = 0.9375 R.
        let want = 0.9375 * peak(&reference);
        assert!(
            (peak(&from_half.drive) - want).abs() < 1e-9,
            "{}",
            peak(&from_half.drive)
        );
        // Without the adoption: 1.25 R + 0.3125 R = 1.5625 R, clamped to 6 dB
        // growth it is still over the half's answer.
        assert!(peak(&unclipped.drive) > peak(&from_half.drive) + 0.1);
        // The change is reported against the drive as played.
        let db = 20.0 * (0.9375 / 0.625f64).log10();
        assert!((from_half.report.drive_change_db - db).abs() < 1e-9);
    }
}
