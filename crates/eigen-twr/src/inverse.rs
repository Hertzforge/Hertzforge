//! The regularized inverse on the RECORD grid: Tikhonov inversion of the sysid
//! FRF, interpolated from the sysid grid onto the (much finer) record grid, plus
//! the per-bin trust data the law's mask is built from.
//!
//! # Grid interpolation
//!
//! H is estimated at `nfft_sysid` (df = fs/nfft, e.g. 4 Hz); the record grid is
//! `n_record` points (df often < 1 Hz). The inverse `G` is interpolated LINEARLY
//! on (re, im) independently: `G` is bounded by construction (the Tikhonov floor
//! caps `|G| <= 1/(2*sqrt(eps))`), so there is no pole to interpolate across, and
//! residual interpolation error is exactly what the iteration absorbs (that is
//! the point of iterating). Coherence interpolates nearest-bin (it is a trust
//! CLASS, not a smooth quantity).
//!
//! # Epsilon policy
//!
//! `eps = (tikhonov_beta_floor * Hmax_inband)^2` with `Hmax` taken over the
//! IN-BAND sysid bins only - out-of-band junk (where the excitation put no
//! energy and |H| is noise) must not set the regularization scale.

use eigen_dsp::fft::Cplx;
use eigen_dsp::frf;
use serde::{Deserialize, Serialize};

use crate::grid::RecordGrid;
use crate::sysid::SysIdResult;
use crate::TwrError;

/// Inversion + trust configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InverseConfig {
    /// Tikhonov floor as a fraction of the in-band peak |H| (`beta` in the
    /// eigen-dsp `suggest_epsilon` sense): plant gain below `beta_floor * Hmax`
    /// is treated as un-driveable. Default 0.05 (26 dB below the peak).
    pub tikhonov_beta_floor: f64,
    /// Per-bin coherence floor for the trust mask: bins identified with
    /// coherence below this get NO correction updates (held, never amplified).
    /// Default 0.9.
    pub coherence_floor: f64,
}

impl Default for InverseConfig {
    fn default() -> Self {
        InverseConfig {
            tikhonov_beta_floor: 0.05,
            coherence_floor: 0.9,
        }
    }
}

impl InverseConfig {
    pub fn validate(&self) -> Result<(), TwrError> {
        if !(self.tikhonov_beta_floor > 0.0 && self.tikhonov_beta_floor < 1.0) {
            return Err(TwrError::Config(
                "tikhonov_beta_floor must be in (0, 1)".into(),
            ));
        }
        if !(self.coherence_floor >= 0.0 && self.coherence_floor <= 1.0) {
            return Err(TwrError::Config("coherence_floor must be in [0, 1]".into()));
        }
        Ok(())
    }
}

/// The regularized inverse resampled onto the record grid.
#[derive(Debug, Clone, PartialEq)]
pub struct GridInverse {
    /// `G[k]` on record-grid bins `0 ..= n/2`.
    pub g: Vec<Cplx>,
    /// Nearest-bin coherence on the record grid (trust input for the mask).
    pub coherence: Vec<f64>,
    /// The Tikhonov epsilon actually used (units |H|^2).
    pub eps: f64,
    /// In-band peak |H| the epsilon was derived from.
    pub hmax_inband: f64,
}

impl GridInverse {
    /// Build from a sysid result for `grid`, regularizing with `cfg` over the
    /// identification band `[f_lo_hz, f_hi_hz]`.
    pub fn from_sysid(
        sysid: &SysIdResult,
        cfg: &InverseConfig,
        grid: &RecordGrid,
        f_lo_hz: f64,
        f_hi_hz: f64,
    ) -> Result<Self, TwrError> {
        cfg.validate()?;
        if (sysid.fs_hz - grid.fs_hz).abs() > 1e-9 {
            return Err(TwrError::Config(format!(
                "sysid fs {} != grid fs {}",
                sysid.fs_hz, grid.fs_hz
            )));
        }
        let df_sysid = sysid.df();
        // In-band peak |H| on the sysid grid.
        let mut hmax = 0.0f64;
        for (k, &(re, im)) in sysid.h.iter().enumerate() {
            let f = k as f64 * df_sysid;
            if f >= f_lo_hz && f <= f_hi_hz {
                hmax = hmax.max((re * re + im * im).sqrt());
            }
        }
        if hmax <= 0.0 {
            return Err(TwrError::SysId(
                "no in-band plant gain identified (|H| = 0 across the band)".into(),
            ));
        }
        let eps = frf::suggest_epsilon(
            &h_band_only(sysid, f_lo_hz, f_hi_hz),
            cfg.tikhonov_beta_floor,
        );

        // CONVENTION BRIDGE (load-bearing - the TWR analog of eigen-dsp's pinned
        // FRF phase sign; do not "simplify"). `h_estimate_hv` returns H in the
        // STANDARD z-domain convention (a lagging plant has negative phase),
        // pinned by eigen-dsp's delay-plant test. But this crate SYNTHESIZES
        // drive spectra with `eigen_dsp::fft`, whose forward transform is the
        // POSITIVE-exponent DFT (see `transform_in_place`: forward theta =
        // +TAU/len); under that transform the multiplier that delays a signal by
        // `d` samples is `e^{+j w d}`, i.e. the CONJUGATE of the standard
        // z-domain H. So the transfer multiplier applicable to spectra built
        // with this crate's fft is `M = conj(H_std)`, and the drive inverse must
        // be built from M, not H_std. Feeding conj(H) into the Tikhonov inverse
        // does exactly that: G = conj(M)/(|M|^2 + eps) with M = conj(H_std).
        // Verified end-to-end by `tests/phase_alignment.rs` (a pure gain+delay
        // plant: the initial drive must land the response ON the target, not
        // 2*delay late). Getting this sign wrong reproduces as an immediate
        // envelope abort with the response ~2*delay late.
        let m: Vec<Cplx> = sysid.h.iter().map(|&(re, im)| (re, -im)).collect();
        let g_sysid = frf::h_inverse_regularized(&m, eps);

        // Interpolate onto the record grid.
        let nb = grid.n_bins();
        let last = sysid.h.len() - 1;
        let mut g = Vec::with_capacity(nb);
        let mut coherence = Vec::with_capacity(nb);
        for k in 0..nb {
            let f = grid.freq(k);
            let pos = f / df_sysid;
            let i0 = (pos.floor() as usize).min(last);
            let i1 = (i0 + 1).min(last);
            let fr = (pos - i0 as f64).clamp(0.0, 1.0);
            let (a, b) = (g_sysid[i0], g_sysid[i1]);
            g.push((a.0 * (1.0 - fr) + b.0 * fr, a.1 * (1.0 - fr) + b.1 * fr));
            let ci = if fr < 0.5 { i0 } else { i1 };
            coherence.push(sysid.coherence[ci]);
        }
        Ok(GridInverse {
            g,
            coherence,
            eps,
            hmax_inband: hmax,
        })
    }
}

/// The sysid FRF restricted to the band (zeros elsewhere) - so `suggest_epsilon`
/// keys off the IN-BAND peak only.
fn h_band_only(sysid: &SysIdResult, f_lo_hz: f64, f_hi_hz: f64) -> Vec<Cplx> {
    let df = sysid.df();
    sysid
        .h
        .iter()
        .enumerate()
        .map(|(k, &h)| {
            let f = k as f64 * df;
            if f >= f_lo_hz && f <= f_hi_hz {
                h
            } else {
                (0.0, 0.0)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysid::SysIdResult;

    /// A synthetic sysid result with H = 2.0 flat in band and garbage outside.
    fn flat_sysid(fs: f64, nfft: usize) -> SysIdResult {
        let nb = nfft / 2 + 1;
        let df = fs / nfft as f64;
        let mut h = Vec::with_capacity(nb);
        let mut c = Vec::with_capacity(nb);
        for k in 0..nb {
            let f = k as f64 * df;
            if (20.0..=2000.0).contains(&f) {
                h.push((2.0, 0.0));
                c.push(0.99);
            } else {
                h.push((250.0, 0.0)); // out-of-band junk that must NOT set eps
                c.push(0.05);
            }
        }
        SysIdResult {
            h,
            coherence: c,
            nfft,
            fs_hz: fs,
            in_band_mean_coherence: 0.99,
        }
    }

    #[test]
    fn eps_keys_off_the_in_band_peak_only() {
        let fs = 8192.0;
        let sysid = flat_sysid(fs, 2048);
        let grid = RecordGrid::new(fs, 6000, 512, 512);
        let inv = GridInverse::from_sysid(&sysid, &InverseConfig::default(), &grid, 20.0, 2000.0)
            .unwrap();
        assert!((inv.hmax_inband - 2.0).abs() < 1e-12);
        let expect = (0.05f64 * 2.0).powi(2);
        assert!((inv.eps - expect).abs() < 1e-12, "eps {}", inv.eps);
    }

    #[test]
    fn in_band_inverse_is_one_over_h_and_interpolates() {
        let fs = 8192.0;
        let sysid = flat_sysid(fs, 2048);
        let grid = RecordGrid::new(fs, 6000, 512, 512);
        let inv = GridInverse::from_sysid(&sysid, &InverseConfig::default(), &grid, 20.0, 2000.0)
            .unwrap();
        assert_eq!(inv.g.len(), grid.n_bins());
        // At 500 Hz (well in-band): G ~ 1/2 (eps = 0.01 vs |H|^2 = 4 -> 2/4.01).
        let k = (500.0 / grid.df()).round() as usize;
        let g = inv.g[k];
        assert!(
            (g.0 - 2.0 / 4.01).abs() < 1e-3 && g.1.abs() < 1e-12,
            "{g:?}"
        );
        // Coherence mapped nearest-bin.
        assert!(inv.coherence[k] > 0.9);
    }

    #[test]
    fn fs_mismatch_is_rejected() {
        let sysid = flat_sysid(8192.0, 2048);
        let grid = RecordGrid::new(4096.0, 6000, 512, 512);
        assert!(
            GridInverse::from_sysid(&sysid, &InverseConfig::default(), &grid, 20.0, 2000.0)
                .is_err()
        );
    }
}
