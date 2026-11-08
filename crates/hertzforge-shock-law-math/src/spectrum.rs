//! One-sided spectra of real records in the `shock-classical` world's
//! convention: the NEGATIVE-exponent transform,
//! `X[k] = sum_n x[n] exp(-j 2 pi k n / N)`, bins `0 ..= N/2`.
//!
//! `hertzforge_dsp::fft` is the positive-exponent transform, which for a real
//! record is the conjugate of this one. [`rfft_half`] conjugates its output and
//! [`irfft_half`] conjugates its input, so the two are exact inverses in the
//! world's convention, and a spectrum formed here is the exact conjugate (the
//! imaginary parts negated, nothing else) of the one time waveform
//! replication forms with its own helpers. The record length must be a power
//! of two; the controller's pulse grid always is.

use crate::Cplx;
use hertzforge_dsp::fft;

/// The one-sided spectrum of the real record `x` (`x.len()` a power of two):
/// `x.len() / 2 + 1` bins.
pub fn rfft_half(x: &[f64]) -> Result<Vec<Cplx>, String> {
    let n = x.len();
    if !n.is_power_of_two() || n < 2 {
        return Err(format!(
            "a record of {n} samples cannot be transformed; it must be a power of two"
        ));
    }
    let mut buf: Vec<Cplx> = x.iter().map(|&v| (v, 0.0)).collect();
    fft::fft_in_place(&mut buf);
    buf.truncate(n / 2 + 1);
    for c in buf.iter_mut() {
        c.1 = -c.1;
    }
    Ok(buf)
}

/// The real record of `n` samples (a power of two) whose one-sided spectrum is
/// `half` (`n / 2 + 1` bins). The imaginary parts of the DC and Nyquist bins
/// are dropped.
pub fn irfft_half(half: &[Cplx], n: usize) -> Result<Vec<f64>, String> {
    if !n.is_power_of_two() || n < 2 {
        return Err(format!(
            "a record of {n} samples cannot be transformed; it must be a power of two"
        ));
    }
    if half.len() != n / 2 + 1 {
        return Err(format!(
            "a spectrum of {} bins is not the half-spectrum of {n} samples ({} bins)",
            half.len(),
            n / 2 + 1
        ));
    }
    // The positive-exponent inverse of the conjugate: bin k takes conj(X[k])
    // and its mirror n - k takes X[k].
    let mut spec = vec![(0.0f64, 0.0f64); n];
    spec[0] = (half[0].0, 0.0);
    spec[n / 2] = (half[n / 2].0, 0.0);
    for k in 1..n / 2 {
        spec[k] = (half[k].0, -half[k].1);
        spec[n - k] = (half[k].0, half[k].1);
    }
    Ok(fft::ifft(&spec).iter().map(|c| c.0).collect())
}

/// `a * b`.
#[inline]
pub fn mul(a: Cplx, b: Cplx) -> Cplx {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

/// `|a|`.
#[inline]
pub fn abs(a: Cplx) -> f64 {
    (a.0 * a.0 + a.1 * a.1).sqrt()
}

/// The conjugate of `a`.
#[inline]
pub fn conj(a: Cplx) -> Cplx {
    (a.0, -a.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delay_has_the_negative_exponent_phase() {
        // A unit impulse at n = 1: X[k] = exp(-j 2 pi k / N), so bin 1 has a
        // NEGATIVE imaginary part (the dsp transform's is positive).
        let mut x = vec![0.0; 16];
        x[1] = 1.0;
        let h = rfft_half(&x).unwrap();
        let w = std::f64::consts::TAU / 16.0;
        assert!((h[1].0 - w.cos()).abs() < 1e-15);
        assert!((h[1].1 + w.sin()).abs() < 1e-15, "{:?}", h[1]);
    }

    #[test]
    fn the_two_transforms_are_inverses_and_refuse_bad_lengths() {
        let x: Vec<f64> = (0..64)
            .map(|i| (i as f64 * 0.37).sin() + 0.2 * (i as f64 * 1.9).cos())
            .collect();
        let back = irfft_half(&rfft_half(&x).unwrap(), 64).unwrap();
        let worst = x
            .iter()
            .zip(&back)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f64, f64::max);
        assert!(worst < 1e-12, "{worst}");
        assert!(rfft_half(&x[..63]).is_err());
        assert!(irfft_half(&[(0.0, 0.0); 32], 64).is_err());
        assert!(irfft_half(&[(0.0, 0.0); 33], 60).is_err());
    }
}
