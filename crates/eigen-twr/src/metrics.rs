//! Convergence metrics + history + debounced divergence detection.
//!
//! # The metrics
//!
//! - `rms_error_pct = 100 * ||y - r_L||_2 / ||r_L||_2` over the padded window
//!   (relative L2 error; the primary gate). Both records live on the same grid,
//!   so residual ring-down in the pads counts as REAL error.
//! - `peak_error_pct = 100 * max|y - r_L| / peak|r_L|`.
//! - `spectral_error_db` = mean over masked bins of `|20*log10(|Y| / |R_L|)|`,
//!   clamped to 60 dB per bin (a zero achieved bin is "very wrong", not
//!   infinitely wrong).
//!
//! Error percent is LEVEL-INVARIANT (both numerator and denominator scale with
//! level), so a gate of 10% means the same thing at -12 dB and 0 dB.
//!
//! # Divergence
//!
//! With plant noise the error converges to a noise-set floor and JITTERS there;
//! a naive "error went up" abort would kill converged runs. Divergence is
//! declared only when the error rose by more than `tol_frac` (relative) on
//! `n_consec` CONSECUTIVE iterations at the SAME level, all while ABOVE the
//! convergence gate.

use eigen_dsp::fft::Cplx;
use serde::{Deserialize, Serialize};

/// The per-iteration error metrics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ErrorMetrics {
    /// `100 * ||y - r_L|| / ||r_L||` (relative L2, the gate metric).
    pub rms_error_pct: f64,
    /// `100 * max|y - r_L| / peak|r_L|`.
    pub peak_error_pct: f64,
    /// Mean masked-bin `|20 log10(|Y|/|R_L|)|`, dB (clamped 60 dB/bin).
    pub spectral_error_db: f64,
    /// Achieved-record RMS (EU).
    pub response_rms: f64,
    /// Drive-record RMS (drive units).
    pub drive_rms: f64,
}

impl ErrorMetrics {
    /// Compute the metrics for one play. `target` / `achieved` are the padded
    /// time windows; `r` / `y` their half-spectra; `mask` the law's trust mask.
    pub fn compute(
        target: &[f64],
        achieved: &[f64],
        drive: &[f64],
        r: &[Cplx],
        y: &[Cplx],
        mask: &[bool],
    ) -> ErrorMetrics {
        assert_eq!(target.len(), achieved.len());
        let mut err2 = 0.0f64;
        let mut ref2 = 0.0f64;
        let mut err_pk = 0.0f64;
        let mut ref_pk = 0.0f64;
        let mut resp2 = 0.0f64;
        for (&t, &a) in target.iter().zip(achieved.iter()) {
            let e = a - t;
            err2 += e * e;
            ref2 += t * t;
            err_pk = err_pk.max(e.abs());
            ref_pk = ref_pk.max(t.abs());
            resp2 += a * a;
        }
        let rms_error_pct = if ref2 > 0.0 {
            100.0 * (err2 / ref2).sqrt()
        } else {
            0.0
        };
        let peak_error_pct = if ref_pk > 0.0 {
            100.0 * err_pk / ref_pk
        } else {
            0.0
        };

        let mut db_sum = 0.0f64;
        let mut db_cnt = 0usize;
        for k in 0..mask.len().min(r.len()).min(y.len()) {
            if !mask[k] {
                continue;
            }
            let rm = (r[k].0 * r[k].0 + r[k].1 * r[k].1).sqrt();
            if rm <= 0.0 {
                continue;
            }
            let ym = (y[k].0 * y[k].0 + y[k].1 * y[k].1).sqrt();
            let db = if ym > 0.0 {
                (20.0 * (ym / rm).log10()).abs().min(60.0)
            } else {
                60.0
            };
            db_sum += db;
            db_cnt += 1;
        }
        ErrorMetrics {
            rms_error_pct,
            peak_error_pct,
            spectral_error_db: if db_cnt > 0 {
                db_sum / db_cnt as f64
            } else {
                0.0
            },
            response_rms: (resp2 / target.len().max(1) as f64).sqrt(),
            drive_rms: eigen_dsp::stats::rms(drive),
        }
    }
}

/// One row of the convergence table.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ConvergenceRecord {
    /// Commanded level, dB relative to full.
    pub level_db: f64,
    /// Iteration number AT this level (1-based).
    pub iteration: u32,
    pub metrics: ErrorMetrics,
    /// Bins the correction clamp limited this pass (loop health).
    pub clamped_bins: usize,
    /// Drive samples the per-sample clip limited this play.
    pub clipped_samples: usize,
}

/// The convergence history - the module's acceptance evidence.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConvergenceHistory {
    pub records: Vec<ConvergenceRecord>,
}

impl ConvergenceHistory {
    pub fn push(&mut self, rec: ConvergenceRecord) {
        self.records.push(rec);
    }

    pub fn last(&self) -> Option<&ConvergenceRecord> {
        self.records.last()
    }

    /// Debounced divergence (see the module docs): the last `n_consec`
    /// TRANSITIONS at level `level_db` each rose by more than `tol_frac`
    /// relative, with every involved record above `gate_pct`.
    pub fn is_diverging(
        &self,
        level_db: f64,
        gate_pct: f64,
        tol_frac: f64,
        n_consec: usize,
    ) -> bool {
        if n_consec == 0 {
            return false;
        }
        let at_level: Vec<&ConvergenceRecord> = self
            .records
            .iter()
            .filter(|r| (r.level_db - level_db).abs() < 1e-9)
            .collect();
        if at_level.len() < n_consec + 1 {
            return false;
        }
        let tail = &at_level[at_level.len() - (n_consec + 1)..];
        tail.windows(2).all(|w| {
            let (prev, cur) = (w[0].metrics.rms_error_pct, w[1].metrics.rms_error_pct);
            prev > gate_pct && cur > gate_pct && cur > prev * (1.0 + tol_frac)
        })
    }

    /// The trace the UI's convergence strip plots: `(overall index, error %)`.
    pub fn error_trace(&self) -> Vec<(f64, f64)> {
        self.records
            .iter()
            .enumerate()
            .map(|(i, r)| (i as f64 + 1.0, r.metrics.rms_error_pct))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(level: f64, iter: u32, err: f64) -> ConvergenceRecord {
        ConvergenceRecord {
            level_db: level,
            iteration: iter,
            metrics: ErrorMetrics {
                rms_error_pct: err,
                peak_error_pct: err,
                spectral_error_db: 0.0,
                response_rms: 1.0,
                drive_rms: 0.1,
            },
            clamped_bins: 0,
            clipped_samples: 0,
        }
    }

    #[test]
    fn metrics_of_a_known_pair() {
        // target = [1, 0, -1, 0], achieved = target + [0.1, 0, 0, 0].
        let t = vec![1.0, 0.0, -1.0, 0.0];
        let a = vec![1.1, 0.0, -1.0, 0.0];
        let d = vec![0.5; 4];
        let m = ErrorMetrics::compute(&t, &a, &d, &[], &[], &[]);
        // ||e|| = 0.1, ||t|| = sqrt(2) -> 7.0710678%
        assert!((m.rms_error_pct - 100.0 * 0.1 / 2f64.sqrt()).abs() < 1e-9);
        assert!((m.peak_error_pct - 10.0).abs() < 1e-9);
        assert!((m.drive_rms - 0.5).abs() < 1e-12);
    }

    #[test]
    fn spectral_error_uses_masked_bins_and_clamps() {
        let t = vec![0.0; 4];
        let a = vec![0.0; 4];
        let d = vec![0.0; 4];
        let r = vec![(1.0, 0.0), (1.0, 0.0), (1.0, 0.0)];
        let y = vec![(2.0, 0.0), (0.0, 0.0), (1.0, 0.0)];
        // Mask only bins 0 and 1: |6.02| dB and the 60 dB clamp -> mean 33.01.
        let mask = vec![true, true, false];
        let m = ErrorMetrics::compute(&t, &a, &d, &r, &y, &mask);
        let expect = (20.0 * 2f64.log10() + 60.0) / 2.0;
        assert!((m.spectral_error_db - expect).abs() < 1e-9);
    }

    #[test]
    fn divergence_needs_consecutive_rises_above_gate() {
        let mut h = ConvergenceHistory::default();
        // Converging then jittering under the gate: never diverging.
        for (i, e) in [50.0, 25.0, 12.0, 8.0, 8.5, 8.2, 8.8].iter().enumerate() {
            h.push(rec(-12.0, i as u32 + 1, *e));
        }
        assert!(!h.is_diverging(-12.0, 10.0, 0.10, 3));

        // Three consecutive >10% rises above the gate: diverging.
        let mut h = ConvergenceHistory::default();
        for (i, e) in [20.0, 24.0, 30.0, 40.0].iter().enumerate() {
            h.push(rec(0.0, i as u32 + 1, *e));
        }
        assert!(h.is_diverging(0.0, 10.0, 0.10, 3));
        // A different level's records do not count.
        assert!(!h.is_diverging(-3.0, 10.0, 0.10, 3));
    }

    #[test]
    fn small_rises_below_tolerance_do_not_trip() {
        let mut h = ConvergenceHistory::default();
        for (i, e) in [20.0, 20.5, 21.0, 21.4].iter().enumerate() {
            h.push(rec(0.0, i as u32 + 1, *e));
        }
        assert!(!h.is_diverging(0.0, 10.0, 0.10, 3));
    }
}
