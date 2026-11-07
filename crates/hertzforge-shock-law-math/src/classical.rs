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

use crate::spectrum::{irfft_half, mul, rfft_half};
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
#[derive(Debug, Clone)]
pub struct FdCorrect {
    params: FdCorrectParams,
    record_len: usize,
    /// The reference's spectrum `R`.
    reference: Vec<Cplx>,
    inverse: Vec<Cplx>,
    trusted: Vec<bool>,
    /// The law's own drive spectrum `D`, kept between calls; empty before the
    /// seed.
    drive_spectrum: Vec<Cplx>,
    /// The last drive's peak, volts (0 before the seed).
    last_peak: f64,
}

impl FdCorrect {
    /// Build the law for a run: the full-level `reference` record
    /// (`record_len` samples, a power of two), and the core's regularized
    /// `inverse` and `trusted` bins on the grid (`record_len / 2 + 1` each).
    pub fn new(
        params: FdCorrectParams,
        record_len: usize,
        reference: &[f64],
        inverse: &[Cplx],
        trusted: &[bool],
    ) -> Result<Self, String> {
        let bins = record_len / 2 + 1;
        if record_len < 2 || !record_len.is_power_of_two() {
            return Err(format!(
                "a record of {record_len} samples is not a pulse grid (a power of two)"
            ));
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
        for (key, v) in [
            ("beta", params.beta),
            ("max_db_per_pulse", params.max_db_per_pulse),
            ("initial_backoff_db", params.initial_backoff_db),
        ] {
            let p = KNOBS.iter().find(|p| p.key == key).expect("declared");
            if !v.is_finite() || v < p.min || v > p.max {
                return Err(format!("{key} is {v}; it runs from {} to {}", p.min, p.max));
            }
        }
        Ok(FdCorrect {
            params,
            record_len,
            reference: rfft_half(reference)?,
            inverse: inverse.to_vec(),
            trusted: trusted.to_vec(),
            drive_spectrum: Vec::new(),
            last_peak: 0.0,
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

    /// The law's drive spectrum `D` (empty before the seed).
    pub fn drive_spectrum(&self) -> &[Cplx] {
        &self.drive_spectrum
    }

    /// The first drive, before any pulse has fired: `D0 = G R` on the trusted
    /// bins, zero elsewhere, then scaled down by `initial_backoff_db`.
    pub fn seed(&mut self) -> Result<Step, String> {
        let scale = 10f64.powf(-self.params.initial_backoff_db / 20.0);
        let mut corrected = 0usize;
        self.drive_spectrum = self
            .reference
            .iter()
            .zip(&self.inverse)
            .zip(&self.trusted)
            .map(|((&r, &g), &t)| {
                if t {
                    corrected += 1;
                    let d = mul(r, g);
                    (d.0 * scale, d.1 * scale)
                } else {
                    (0.0, 0.0)
                }
            })
            .collect();
        let drive = irfft_half(&self.drive_spectrum, self.record_len)?;
        self.last_peak = peak(&drive);
        Ok(Step {
            drive,
            report: StepReport {
                corrected_bins: corrected,
                clamped_bins: 0,
                drive_change_db: 0.0,
            },
        })
    }

    /// The next drive, from the last pulse's `response` referred to full level
    /// (`record_len` samples): `D += beta G (R - Y)` on the trusted bins, each
    /// bin's magnitude allowed to grow by at most `max_db_per_pulse` (shrinking
    /// is always allowed).
    pub fn iterate(&mut self, response: &[f64]) -> Result<Step, String> {
        if response.len() != self.record_len {
            return Err(format!(
                "the response has {} samples; the record has {}",
                response.len(),
                self.record_len
            ));
        }
        if self.drive_spectrum.is_empty() {
            return Err("the law was asked for a correction before its seed".into());
        }
        if response.iter().any(|v| !v.is_finite()) {
            return Err("the response holds a value that is not a finite number".into());
        }
        let y = rfft_half(response)?;
        let beta = self.params.beta;
        let grow = 10f64.powf(self.params.max_db_per_pulse / 20.0);
        let (mut corrected, mut clamped) = (0usize, 0usize);
        // Indexed, as time waveform replication's loop is: five arrays move
        // together bin by bin and the index is the clearest name for that.
        #[allow(clippy::needless_range_loop)]
        for k in 0..self.drive_spectrum.len() {
            if !self.trusted[k] {
                continue;
            }
            // The same arithmetic, in the same order, as time waveform
            // replication's correction.
            let (er, ei) = (self.reference[k].0 - y[k].0, self.reference[k].1 - y[k].1);
            let (gr, gi) = self.inverse[k];
            let dr = beta * (er * gr - ei * gi);
            let di = beta * (er * gi + ei * gr);
            let old = self.drive_spectrum[k];
            let old_mag = (old.0 * old.0 + old.1 * old.1).sqrt();
            let mut new = (old.0 + dr, old.1 + di);
            let new_mag = (new.0 * new.0 + new.1 * new.1).sqrt();
            let cap = old_mag.max(1e-15) * grow;
            if new_mag > cap {
                let s = cap / new_mag;
                new = (new.0 * s, new.1 * s);
                clamped += 1;
            }
            if new != old {
                corrected += 1;
            }
            self.drive_spectrum[k] = new;
        }
        let drive = irfft_half(&self.drive_spectrum, self.record_len)?;
        let p = peak(&drive);
        let drive_change_db = if self.last_peak > 0.0 && p > 0.0 {
            20.0 * (p / self.last_peak).log10()
        } else {
            0.0
        };
        self.last_peak = p;
        Ok(Step {
            drive,
            report: StepReport {
                corrected_bins: corrected,
                clamped_bins: clamped,
                drive_change_db,
            },
        })
    }
}

fn peak(x: &[f64]) -> f64 {
    x.iter().fold(0.0f64, |m, v| m.max(v.abs()))
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
    fn new_refuses_mismatched_lengths_and_out_of_range_values() {
        let r = vec![0.0; 8];
        let g = vec![(1.0, 0.0); 5];
        let t = vec![true; 5];
        assert!(FdCorrect::new(FdCorrectParams::default(), 8, &r[..7], &g, &t).is_err());
        assert!(FdCorrect::new(FdCorrectParams::default(), 8, &r, &g[..4], &t).is_err());
        assert!(FdCorrect::new(FdCorrectParams::default(), 6, &r[..6], &g[..4], &t[..4]).is_err());
        let bad = FdCorrectParams {
            beta: 2.0,
            ..FdCorrectParams::default()
        };
        assert!(FdCorrect::new(bad, 8, &r, &g, &t)
            .unwrap_err()
            .contains("beta"));
        let mut law = FdCorrect::new(FdCorrectParams::default(), 8, &r, &g, &t).unwrap();
        assert!(law.iterate(&r).unwrap_err().contains("before its seed"));
        law.seed().unwrap();
        assert!(law.iterate(&r[..3]).unwrap_err().contains("samples"));
        let mut nan = r.clone();
        nan[2] = f64::NAN;
        assert!(law.iterate(&nan).unwrap_err().contains("finite"));
    }

    /// A unity plant whose inverse is exact: the seed is the reference, and
    /// each pulse halves the error at beta 0.5.
    #[test]
    fn on_a_unity_plant_the_seed_is_the_reference_and_each_pulse_halves_the_error() {
        let n = 64;
        let reference: Vec<f64> = (0..n)
            .map(|i| {
                let t = i as f64 / n as f64;
                if (0.25..0.5).contains(&t) {
                    (std::f64::consts::PI * (t - 0.25) / 0.25).sin()
                } else {
                    0.0
                }
            })
            .collect();
        let bins = n / 2 + 1;
        let g = vec![(1.0, 0.0); bins];
        let t = vec![true; bins];
        let mut law = FdCorrect::new(FdCorrectParams::default(), n, &reference, &g, &t).unwrap();
        let seed = law.seed().unwrap();
        let err = |d: &[f64]| {
            d.iter()
                .zip(&reference)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f64, f64::max)
        };
        assert!(err(&seed.drive) < 1e-12);
        assert_eq!(seed.report.corrected_bins, bins);
        // The plant measures 0.8 of the drive: each pulse removes half the
        // remaining 20% shortfall.
        let mut drive = seed.drive;
        let mut short = 0.2;
        for _ in 0..3 {
            let response: Vec<f64> = drive.iter().map(|v| 0.8 * v).collect();
            let next = law.iterate(&response).unwrap();
            short *= 1.0 - 0.5 * 0.8;
            let gain = peak(&next.drive) / peak(&reference) * 0.8;
            assert!((1.0 - gain - short).abs() < 1e-9, "{gain} {short}");
            assert!(next.report.drive_change_db > 0.0);
            drive = next.drive;
        }
    }

    #[test]
    fn the_backoff_scales_the_seed_and_untrusted_bins_stay_at_zero() {
        let n = 32;
        let reference: Vec<f64> = (0..n).map(|i| ((i as f64) * 0.7).sin()).collect();
        let bins = n / 2 + 1;
        let g = vec![(0.5, 0.25); bins];
        let mut t = vec![true; bins];
        t[3] = false;
        let p = FdCorrectParams {
            initial_backoff_db: 6.0,
            ..FdCorrectParams::default()
        };
        let mut backed = FdCorrect::new(p, n, &reference, &g, &t).unwrap();
        let mut full = FdCorrect::new(FdCorrectParams::default(), n, &reference, &g, &t).unwrap();
        let (a, b) = (backed.seed().unwrap(), full.seed().unwrap());
        let ratio = peak(&a.drive) / peak(&b.drive);
        assert!((ratio - 10f64.powf(-6.0 / 20.0)).abs() < 1e-12, "{ratio}");
        assert_eq!(backed.drive_spectrum()[3], (0.0, 0.0));
        let zeros = vec![0.0; n];
        backed.iterate(&zeros).unwrap();
        assert_eq!(
            backed.drive_spectrum()[3],
            (0.0, 0.0),
            "a held bin never moves"
        );
    }

    #[test]
    fn a_bin_rises_at_most_its_clamp_per_pulse_and_may_always_fall() {
        let n = 16;
        let mut reference = vec![0.0; n];
        reference[4] = 1.0;
        let bins = n / 2 + 1;
        let g = vec![(1e-3, 0.0); bins];
        let t = vec![true; bins];
        let p = FdCorrectParams {
            beta: 1.0,
            ..FdCorrectParams::default()
        };
        let mags = |l: &FdCorrect| -> Vec<f64> {
            l.drive_spectrum()
                .iter()
                .map(|&c| crate::spectrum::abs(c))
                .collect()
        };
        // A zero response asks each bin to double (6.02 dB): over the 6 dB
        // clamp, so every bin with reference content is held to 6 dB.
        let mut law = FdCorrect::new(p, n, &reference, &g, &t).unwrap();
        law.seed().unwrap();
        let before = mags(&law);
        let step = law.iterate(&vec![0.0; n]).unwrap();
        assert_eq!(step.report.clamped_bins, bins);
        let cap = 10f64.powf(6.0 / 20.0);
        for (a, b) in mags(&law).iter().zip(&before) {
            assert!((a / b - cap).abs() < 1e-12, "{a} {b}");
        }
        // A response twice the reference shrinks every bin, unclamped.
        let over: Vec<f64> = reference.iter().map(|v| 2.0 * v).collect();
        let before = mags(&law);
        let step = law.iterate(&over).unwrap();
        assert_eq!(step.report.clamped_bins, 0);
        assert!(mags(&law).iter().zip(&before).all(|(a, b)| a < b));
        assert!(step.report.drive_change_db < 0.0);
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
