//! The TWR control law - pure spectral functions, no backend anywhere near them.
//!
//! ```text
//! init:    D_0[k]     = G[k] * R_L[k]                       (masked bins)
//! iterate: D_{n+1}[k] = D_n[k] + beta * G[k] * (R_L[k] - Y_n[k])
//! ```
//!
//! # Convergence (the beta math)
//!
//! Per bin the error propagates as `e_{n+1} = (1 - beta*H*G) * e_n`. With a
//! perfect estimate, `H*G = |H|^2 / (|H|^2 + eps)` is real in (0, 1], so any
//! `beta` in (0, 2) converges; with magnitude error `c = |H*G|` and phase error
//! `phi` the condition is `beta * c < 2*cos(phi)` - beyond 90 degrees of phase
//! error NO positive beta converges, which is why the trust mask (coherence
//! floor) excludes bins whose phase estimate cannot be trusted rather than
//! letting any beta "handle" them. Default `beta = 0.5` halves a
//! well-identified bin's error per pass and leaves 4x margin against magnitude
//! error at zero phase error.
//!
//! # The mask (one mechanism per failure mode)
//!
//! A bin receives correction updates only if ALL of:
//! - in the correction band `[f_lo, f_hi]` (DC/Nyquist excluded by `f_lo > 0`),
//! - sysid coherence at the bin >= `coherence_floor` (phase trust),
//! - `|R_L[k]|` within `ref_floor_db` of the in-band target spectral peak
//!   (do not chase bins whose target is below the noise floor - they would
//!   integrate noise into the drive record).
//!
//! Unmasked bins HOLD their drive value (never amplified, never zeroed after
//! init - the initial drive is zero there by construction).
//!
//! # The per-iteration clamp
//!
//! `max_db_per_iter` bounds each bin's MAGNITUDE GROWTH per pass (shrinking is
//! always allowed - toward less drive is the safe direction). This bounds any
//! transient the masks let through (the divergence guard's companion).

use eigen_dsp::fft::Cplx;
use serde::{Deserialize, Serialize};

use crate::grid::RecordGrid;
use crate::inverse::GridInverse;
use crate::TwrError;

/// Control-law knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawConfig {
    /// The relaxation / convergence gain `beta` in (0, 1]. Default 0.5.
    /// Values above 1 are rejected (per-bin overshoot risk under
    /// FRF error; commercial packages cap their update ratio at or below 0.5).
    pub beta: f64,
    /// Per-bin magnitude-growth clamp per iteration, dB. Default 6.
    pub max_db_per_iter: f64,
    /// Reference dynamic-range floor, dB below the in-band target spectral
    /// peak; bins below it are excluded from correction. Default 50.
    pub ref_floor_db: f64,
    /// Correction band low edge, Hz (> 0; also the sysid band by default).
    pub f_lo_hz: f64,
    /// Correction band high edge, Hz.
    pub f_hi_hz: f64,
    /// Conservative initial-drive backoff, dB (>= 0). The FIRST drive is
    /// `D_0 = G * R_L` scaled DOWN by this much; the iteration recovers the
    /// level. Real plants are uncertain at sysid quality, so commercial TWR
    /// packages start the first play under the prediction rather than at it -
    /// undershoot is corrected upward safely, overshoot is an overtest.
    /// Default 0 (trust the inverse; the Sim plant is linear).
    pub initial_drive_backoff_db: f64,
    /// Minimum fraction of the TOTAL target spectral energy the trust mask must
    /// cover, or the run aborts ("reference content outside the correctable
    /// band"). Unmasked bins are never corrected, so their target energy is a
    /// PERMANENT error floor: rms_error_pct can never fall below
    /// `100 * sqrt(1 - coverage)`. Default 0.9 (error floor <= ~32%); raise it
    /// when the gate is tight - gating at 10% NEEDS coverage >= 0.99.
    pub min_masked_energy_frac: f64,
}

impl Default for LawConfig {
    fn default() -> Self {
        LawConfig {
            beta: 0.5,
            max_db_per_iter: 6.0,
            ref_floor_db: 50.0,
            f_lo_hz: 20.0,
            f_hi_hz: 2000.0,
            initial_drive_backoff_db: 0.0,
            min_masked_energy_frac: 0.9,
        }
    }
}

impl LawConfig {
    pub fn validate(&self, fs_hz: f64) -> Result<(), TwrError> {
        if self.beta.is_nan() || self.beta <= 0.0 || self.beta > 1.0 {
            return Err(TwrError::Config(format!(
                "beta must be in (0, 1] (got {})",
                self.beta
            )));
        }
        if self.max_db_per_iter.is_nan() || self.max_db_per_iter <= 0.0 {
            return Err(TwrError::Config("max_db_per_iter must be positive".into()));
        }
        if self.ref_floor_db.is_nan() || self.ref_floor_db <= 0.0 {
            return Err(TwrError::Config("ref_floor_db must be positive".into()));
        }
        if self.f_lo_hz.is_nan()
            || self.f_hi_hz.is_nan()
            || self.f_lo_hz <= 0.0
            || self.f_hi_hz <= self.f_lo_hz
            || self.f_hi_hz >= fs_hz / 2.0
        {
            return Err(TwrError::Config(format!(
                "correction band [{}, {}] Hz invalid for fs {}",
                self.f_lo_hz, self.f_hi_hz, fs_hz
            )));
        }
        if self.initial_drive_backoff_db.is_nan() || self.initial_drive_backoff_db < 0.0 {
            return Err(TwrError::Config(
                "initial_drive_backoff_db must be >= 0".into(),
            ));
        }
        if self.min_masked_energy_frac.is_nan()
            || self.min_masked_energy_frac < 0.0
            || self.min_masked_energy_frac >= 1.0
        {
            return Err(TwrError::Config(
                "min_masked_energy_frac must be in [0, 1)".into(),
            ));
        }
        Ok(())
    }
}

/// Build the per-bin trust mask for a level-scaled target spectrum `r` on
/// `grid` (see the module docs for the three conditions).
pub fn build_mask(r: &[Cplx], grid: &RecordGrid, inv: &GridInverse, law: &LawConfig) -> Vec<bool> {
    let nb = grid.n_bins();
    assert_eq!(r.len(), nb, "target spectrum bins != grid bins");
    assert_eq!(inv.g.len(), nb, "inverse bins != grid bins");
    // In-band target spectral peak.
    let mut rmax = 0.0f64;
    for (k, &(re, im)) in r.iter().enumerate() {
        let f = grid.freq(k);
        if f >= law.f_lo_hz && f <= law.f_hi_hz {
            rmax = rmax.max((re * re + im * im).sqrt());
        }
    }
    let floor = rmax * 10f64.powf(-law.ref_floor_db / 20.0);
    (0..nb)
        .map(|k| {
            let f = grid.freq(k);
            if !(f >= law.f_lo_hz && f <= law.f_hi_hz) {
                return false;
            }
            let (re, im) = r[k];
            let mag = (re * re + im * im).sqrt();
            mag >= floor
        })
        .collect()
}

/// Refine `mask` with the coherence floor (kept separate from [`build_mask`] so
/// tests can exercise the two gates independently; the service composes both).
pub fn apply_coherence_gate(mask: &mut [bool], inv: &GridInverse, coherence_floor: f64) {
    for (m, &c) in mask.iter_mut().zip(inv.coherence.iter()) {
        if c < coherence_floor {
            *m = false;
        }
    }
}

/// The fraction of TOTAL target spectral energy covered by `mask`. Unmasked
/// target energy is a permanent error floor (never corrected), so the service
/// gates on this at sysid time (`min_masked_energy_frac`) and the UI displays
/// it as loop health.
pub fn mask_energy_coverage(r: &[Cplx], mask: &[bool]) -> f64 {
    let mut total = 0.0f64;
    let mut covered = 0.0f64;
    for (k, &(re, im)) in r.iter().enumerate() {
        let e = re * re + im * im;
        total += e;
        if mask.get(k).copied().unwrap_or(false) {
            covered += e;
        }
    }
    if total > 0.0 {
        covered / total
    } else {
        0.0
    }
}

/// The initial drive spectrum: `D_0[k] = G[k] * R_L[k]` on masked bins, zero
/// elsewhere.
pub fn initial_drive(r: &[Cplx], inv: &GridInverse, mask: &[bool]) -> Vec<Cplx> {
    r.iter()
        .zip(inv.g.iter())
        .zip(mask.iter())
        .map(|((&(rr, ri), &(gr, gi)), &m)| {
            if m {
                (rr * gr - ri * gi, rr * gi + ri * gr)
            } else {
                (0.0, 0.0)
            }
        })
        .collect()
}

/// One correction pass: `D[k] += beta * G[k] * (R_L[k] - Y[k])` on masked bins,
/// with the per-iteration magnitude-growth clamp. Returns the number of bins
/// whose growth hit the clamp (loop-health telemetry).
pub fn correct(
    d: &mut [Cplx],
    r: &[Cplx],
    y: &[Cplx],
    inv: &GridInverse,
    mask: &[bool],
    law: &LawConfig,
) -> usize {
    assert_eq!(d.len(), r.len());
    assert_eq!(y.len(), r.len());
    let grow = 10f64.powf(law.max_db_per_iter / 20.0);
    let mut clamped = 0usize;
    for k in 0..d.len() {
        if !mask[k] {
            continue;
        }
        let (er, ei) = (r[k].0 - y[k].0, r[k].1 - y[k].1);
        let (gr, gi) = inv.g[k];
        // delta = beta * G * E
        let dr = law.beta * (er * gr - ei * gi);
        let di = law.beta * (er * gi + ei * gr);
        let old = d[k];
        let old_mag = (old.0 * old.0 + old.1 * old.1).sqrt();
        let mut new = (old.0 + dr, old.1 + di);
        let new_mag = (new.0 * new.0 + new.1 * new.1).sqrt();
        // Growth clamp: magnitude may not grow by more than `grow` per pass.
        // Shrinking is always allowed (less drive = the safe direction). A tiny
        // absolute base un-sticks exact-zero bins the mask admitted.
        let cap = old_mag.max(1e-15) * grow;
        if new_mag > cap {
            let s = cap / new_mag;
            new = (new.0 * s, new.1 * s);
            clamped += 1;
        }
        d[k] = new;
    }
    clamped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::RecordGrid;
    use crate::inverse::GridInverse;

    /// A grid + flat plant fixture: H = 2, G = Tikhonov(2), coherence 0.99.
    fn fixture(nb_coh: Option<(usize, f64)>) -> (RecordGrid, GridInverse) {
        let grid = RecordGrid::new(8192.0, 3000, 500, 500); // n = 4096
        let nb = grid.n_bins();
        let eps = 0.01f64; // (0.05 * 2)^2
        let g_val = 2.0 / (4.0 + eps);
        let mut coherence = vec![0.99f64; nb];
        if let Some((k, c)) = nb_coh {
            coherence[k] = c;
        }
        (
            grid.clone(),
            GridInverse {
                g: vec![(g_val, 0.0); nb],
                coherence,
                eps,
                hmax_inband: 2.0,
            },
        )
    }

    fn law() -> LawConfig {
        LawConfig::default()
    }

    #[test]
    fn beta_over_one_rejected() {
        let mut l = law();
        l.beta = 1.5;
        assert!(l.validate(8192.0).is_err());
        l.beta = 1.0;
        assert!(l.validate(8192.0).is_ok());
    }

    #[test]
    fn mask_excludes_out_of_band_and_below_floor() {
        let (grid, inv) = fixture(None);
        let nb = grid.n_bins();
        let mut r = vec![(0.0, 0.0); nb];
        // Strong target at 500 Hz, weak (below -50 dB) at 800 Hz, one at 3 kHz
        // (out of the 20-2000 band).
        let k500 = (500.0 / grid.df()).round() as usize;
        let k800 = (800.0 / grid.df()).round() as usize;
        let k3000 = (3000.0 / grid.df()).round() as usize;
        r[k500] = (1.0, 0.0);
        r[k800] = (1e-4, 0.0); // -80 dB vs peak
        r[k3000] = (1.0, 0.0);
        let mask = build_mask(&r, &grid, &inv, &law());
        assert!(mask[k500]);
        assert!(!mask[k800], "below the ref floor must be excluded");
        assert!(!mask[k3000], "out of band must be excluded");
        assert!(!mask[0], "DC must be excluded");
        assert!(!mask[nb - 1], "Nyquist must be excluded");
    }

    #[test]
    fn coherence_gate_holds_bins() {
        let k = 250usize;
        let (grid, inv) = fixture(Some((k, 0.2)));
        let nb = grid.n_bins();
        let f = grid.freq(k);
        assert!(f > 20.0 && f < 2000.0);
        let mut r = vec![(0.0, 0.0); nb];
        r[k] = (1.0, 0.0);
        let mut mask = build_mask(&r, &grid, &inv, &law());
        assert!(mask[k], "bin passes the target-floor mask");
        apply_coherence_gate(&mut mask, &inv, 0.9);
        assert!(!mask[k], "low-coherence bin must be held");
        // A held bin's drive never moves.
        let mut d = initial_drive(&r, &inv, &mask);
        assert_eq!(d[k], (0.0, 0.0));
        let y = vec![(0.0, 0.0); nb];
        correct(&mut d, &r, &y, &inv, &mask, &law());
        assert_eq!(d[k], (0.0, 0.0), "held bin was amplified");
    }

    #[test]
    fn one_pass_converges_a_perfect_plant_bin_by_beta() {
        // With response Y = H * D and perfect G: e_{n+1} = (1 - beta*H*G)*e_n.
        let (grid, inv) = fixture(None);
        let nb = grid.n_bins();
        let k = (500.0 / grid.df()).round() as usize;
        let mut r = vec![(0.0, 0.0); nb];
        r[k] = (1.0, 0.5);
        let mask = build_mask(&r, &grid, &inv, &law());
        let mut d = initial_drive(&r, &inv, &mask);

        let h = (2.0, 0.0);
        let simulate = |d: &[Cplx]| -> Vec<Cplx> {
            d.iter()
                .map(|&(re, im)| (re * h.0 - im * h.1, re * h.1 + im * h.0))
                .collect()
        };
        let err_mag = |d: &[Cplx]| -> f64 {
            let y = simulate(d);
            let (er, ei) = (r[k].0 - y[k].0, r[k].1 - y[k].1);
            (er * er + ei * ei).sqrt()
        };

        let hg = 2.0 * inv.g[k].0; // real, just under 1 (Tikhonov)
        let factor_expect = 1.0 - 0.5 * hg;
        let e0 = err_mag(&d);
        let y0 = simulate(&d);
        correct(&mut d, &r, &y0, &inv, &mask, &law());
        let e1 = err_mag(&d);
        assert!(
            (e1 / e0 - factor_expect).abs() < 1e-9,
            "convergence factor {} vs expected {}",
            e1 / e0,
            factor_expect
        );
    }

    #[test]
    fn clamp_bounds_growth_per_pass() {
        let (grid, inv) = fixture(None);
        let nb = grid.n_bins();
        let k = (500.0 / grid.df()).round() as usize;
        let mut r = vec![(0.0, 0.0); nb];
        r[k] = (1.0, 0.0);
        let mask = build_mask(&r, &grid, &inv, &law());
        // Start from a TINY drive at the bin; a huge error wants a huge jump.
        let mut d = vec![(0.0, 0.0); nb];
        d[k] = (1e-6, 0.0);
        let y = vec![(0.0, 0.0); nb]; // zero response -> full error
        let clamped = correct(&mut d, &r, &y, &inv, &mask, &law());
        assert!(clamped >= 1);
        let mag = (d[k].0 * d[k].0 + d[k].1 * d[k].1).sqrt();
        let cap = 1e-6 * 10f64.powf(6.0 / 20.0);
        assert!(mag <= cap * (1.0 + 1e-12), "mag {mag} exceeded cap {cap}");
    }

    #[test]
    fn shrinking_is_never_clamped() {
        let (grid, inv) = fixture(None);
        let nb = grid.n_bins();
        let k = (500.0 / grid.df()).round() as usize;
        let mut r = vec![(0.0, 0.0); nb];
        r[k] = (0.1, 0.0);
        let mask = build_mask(&r, &grid, &inv, &law());
        // Overdriven bin: response far above target -> correction shrinks drive.
        let mut d = vec![(0.0, 0.0); nb];
        d[k] = (10.0, 0.0);
        let mut y = vec![(0.0, 0.0); nb];
        y[k] = (20.0, 0.0);
        let clamped = correct(&mut d, &r, &y, &inv, &mask, &law());
        assert_eq!(clamped, 0);
        let mag = (d[k].0 * d[k].0 + d[k].1 * d[k].1).sqrt();
        assert!(mag < 10.0, "drive should have shrunk, got {mag}");
    }
}
