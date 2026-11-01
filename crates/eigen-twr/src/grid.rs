//! The padded working grid + half-spectrum FFT helpers.
//!
//! # Why a padded grid
//!
//! The correction law works on whole-record FFTs, which treat the play as
//! PERIODIC; the real plant convolves LINEARLY and rings past the record end.
//! If the FFT length equaled the section length, late response energy would wrap
//! onto the record head and the "error" there would be a lie, biasing every
//! iteration. So the working grid pads the section on both sides:
//!
//! ```text
//! index:   0 .. pad_pre .. pad_pre+section_len .. n (= next_pow2)
//! target:  0        r_L(t)                      0
//! drive:   free     free                        free
//! ```
//!
//! The target is ZERO in the pads; the drive is FREE everywhere (the law may put
//! pre-ring before the section to pre-compensate plant delay, and anti-ring after
//! it to cancel residual ring-down - both are real, physical corrections). The
//! player plays the whole `n`-sample drive and returns the whole `n`-sample
//! response, so target and response always compare on the same grid.
//!
//! # Half-spectrum convention
//!
//! Real signals only, so spectra are stored single-sided: bins `0 ..= n/2`
//! (length `n/2 + 1`), matching `eigen_dsp::frf`'s convention and this crate's
//! FFT sign (negative-exponent forward - see the load-bearing phase note on
//! `eigen_dsp::frf::h_estimate_hv`). [`irfft_half`] rebuilds the Hermitian
//! mirror; DC and Nyquist imaginary parts are forced to zero (both bins are
//! outside any valid control band - config validation enforces `f_lo > 0`).

use eigen_dsp::fft::{self, Cplx};
use serde::{Deserialize, Serialize};

/// The padded working grid. Immutable per prepared run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordGrid {
    /// Playout sample rate, Hz.
    pub fs_hz: f64,
    /// Total padded window length (power of two).
    pub n: usize,
    /// Quiet pad before the section (drive pre-ring room + plant-delay slack).
    pub pad_pre: usize,
    /// Section (the replicated part of the reference) length in samples.
    pub section_len: usize,
}

impl RecordGrid {
    /// Build a grid for `section_len` samples at `fs_hz` with `pad_pre` /
    /// `pad_post` quiet samples around it; `n` is the next power of two.
    pub fn new(fs_hz: f64, section_len: usize, pad_pre: usize, pad_post: usize) -> Self {
        let n = fft::next_pow2(section_len + pad_pre + pad_post);
        RecordGrid {
            fs_hz,
            n,
            pad_pre,
            section_len,
        }
    }

    /// Number of half-spectrum bins (`n/2 + 1`).
    pub fn n_bins(&self) -> usize {
        self.n / 2 + 1
    }

    /// Frequency of half-spectrum bin `k`, Hz.
    pub fn freq(&self, k: usize) -> f64 {
        k as f64 * self.fs_hz / self.n as f64
    }

    /// Bin spacing, Hz.
    pub fn df(&self) -> f64 {
        self.fs_hz / self.n as f64
    }

    /// Lay `section` onto the padded window (zeros outside). Panics if the
    /// section does not fit (the constructor sizes `n` so it always does).
    pub fn window_from_section(&self, section: &[f64]) -> Vec<f64> {
        assert!(
            self.pad_pre + section.len() <= self.n,
            "section does not fit the grid"
        );
        let mut w = vec![0.0f64; self.n];
        w[self.pad_pre..self.pad_pre + section.len()].copy_from_slice(section);
        w
    }
}

/// Forward FFT of a real window onto the half-spectrum (bins `0..=n/2`).
/// `x.len()` must equal `n` (the grid window length, a power of two).
pub fn rfft_half(x: &[f64], n: usize) -> Vec<Cplx> {
    assert_eq!(x.len(), n, "rfft_half: window length must equal grid n");
    assert!(n.is_power_of_two(), "rfft_half: n must be a power of two");
    let mut buf: Vec<Cplx> = x.iter().map(|&v| (v, 0.0)).collect();
    fft::fft_in_place(&mut buf);
    buf.truncate(n / 2 + 1);
    buf
}

/// Inverse FFT of a half-spectrum back to a real `n`-sample window. Rebuilds the
/// Hermitian mirror; DC and Nyquist imaginary parts are dropped (forced real).
pub fn irfft_half(half: &[Cplx], n: usize) -> Vec<f64> {
    assert!(n.is_power_of_two(), "irfft_half: n must be a power of two");
    assert_eq!(
        half.len(),
        n / 2 + 1,
        "irfft_half: half-spectrum must have n/2+1 bins"
    );
    let mut spec = vec![(0.0f64, 0.0f64); n];
    spec[0] = (half[0].0, 0.0);
    spec[n / 2] = (half[n / 2].0, 0.0);
    for k in 1..n / 2 {
        spec[k] = half[k];
        spec[n - k] = (half[k].0, -half[k].1);
    }
    fft::ifft(&spec).iter().map(|c| c.0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_sizes_to_next_pow2_and_places_section() {
        let g = RecordGrid::new(8192.0, 6000, 512, 512);
        assert_eq!(g.n, 8192); // 6000 + 1024 = 7024 -> 8192
        assert_eq!(g.n_bins(), 4097);
        let w = g.window_from_section(&vec![1.0; 6000]);
        assert_eq!(w.len(), 8192);
        assert_eq!(w[511], 0.0);
        assert_eq!(w[512], 1.0);
        assert_eq!(w[512 + 5999], 1.0);
        assert_eq!(w[512 + 6000], 0.0);
    }

    #[test]
    fn rfft_irfft_round_trips_a_real_window() {
        let n = 1024;
        let g = RecordGrid::new(1024.0, 800, 100, 100);
        assert_eq!(g.n, n);
        // A few in-band tones plus noise-ish content.
        let x: Vec<f64> = (0..n)
            .map(|i| {
                let t = i as f64 / 1024.0;
                (std::f64::consts::TAU * 50.0 * t).sin()
                    + 0.3 * (std::f64::consts::TAU * 173.0 * t).cos()
            })
            .collect();
        let half = rfft_half(&x, n);
        let back = irfft_half(&half, n);
        let max_err = x
            .iter()
            .zip(back.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f64, f64::max);
        assert!(max_err < 1e-9, "round-trip error {max_err}");
    }

    #[test]
    fn irfft_forces_edge_bins_real() {
        let n = 64;
        let mut half = vec![(0.0, 0.0); n / 2 + 1];
        half[0] = (1.0, 123.0); // bogus imaginary DC
        half[n / 2] = (0.5, -7.0); // bogus imaginary Nyquist
        let x = irfft_half(&half, n);
        // A real DC + Nyquist pair produces a strictly real, finite window.
        assert!(x.iter().all(|v| v.is_finite()));
        // DC of 1.0 -> mean 1/64 * ... spot-check the mean is the DC/n value.
        let mean = x.iter().sum::<f64>() / n as f64;
        assert!((mean - 1.0 / n as f64).abs() < 1e-12);
    }

    #[test]
    fn freq_axis_matches_df() {
        let g = RecordGrid::new(8192.0, 3000, 500, 500);
        assert_eq!(g.n, 4096);
        assert!((g.df() - 2.0).abs() < 1e-12);
        assert!((g.freq(100) - 200.0).abs() < 1e-12);
    }
}
