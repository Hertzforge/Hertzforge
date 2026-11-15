//! Spectra of records in this crate's convention (the negative-exponent
//! one-sided transform), and the per-line products the laws need.
//!
//! `hertzforge_dsp::fft` is the positive-exponent transform; for a real record
//! it is the conjugate of this one, so [`rfft`] conjugates its output and
//! [`irfft`] conjugates its input.

use crate::{MatrixStack, C64};
use hertzforge_dsp::fft;

/// The one-sided spectrum of a real record whose length is a power of two:
/// `x.len() / 2 + 1` lines.
pub fn rfft(x: &[f64]) -> Result<Vec<C64>, String> {
    let n = x.len();
    if n < 2 || !n.is_power_of_two() {
        return Err(format!("a record of {n} samples is not a power of two"));
    }
    let mut buf: Vec<(f64, f64)> = x.iter().map(|&v| (v, 0.0)).collect();
    fft::fft_in_place(&mut buf);
    Ok(buf[..n / 2 + 1]
        .iter()
        .map(|c| C64::new(c.0, -c.1))
        .collect())
}

/// The real record of `n` samples whose one-sided spectrum is `half`. The
/// imaginary parts of the zero and Nyquist lines are dropped.
pub fn irfft(half: &[C64], n: usize) -> Result<Vec<f64>, String> {
    if n < 2 || !n.is_power_of_two() || half.len() != n / 2 + 1 {
        return Err(format!(
            "{} lines are not the half-spectrum of a power-of-two record of {n} samples",
            half.len()
        ));
    }
    let mut spec = vec![(0.0f64, 0.0f64); n];
    spec[0] = (half[0].re, 0.0);
    spec[n / 2] = (half[n / 2].re, 0.0);
    for k in 1..n / 2 {
        spec[k] = (half[k].re, -half[k].im);
        spec[n - k] = (half[k].re, half[k].im);
    }
    Ok(fft::ifft(&spec).iter().map(|c| c.0).collect())
}

/// The spectrum of every row, each divided by `scale`.
pub fn rows_rfft(rows: &[Vec<f64>], scale: f64) -> Result<Vec<Vec<C64>>, String> {
    rows.iter()
        .map(|r| Ok(rfft(r)?.into_iter().map(|v| v / scale).collect()))
        .collect()
}

/// The record of every spectrum row, each multiplied by `scale`.
pub fn rows_irfft(spectra: &[Vec<C64>], n: usize, scale: f64) -> Result<Vec<Vec<f64>>, String> {
    spectra
        .iter()
        .map(|s| Ok(irfft(s, n)?.into_iter().map(|v| v * scale).collect()))
        .collect()
}

/// Angular frequency of every line, rad/s.
pub fn omegas(fs_hz: f64, record_len: usize) -> Vec<f64> {
    let df = fs_hz / record_len as f64;
    (0..record_len / 2 + 1)
        .map(|k| std::f64::consts::TAU * k as f64 * df)
        .collect()
}

/// `out[r][f] = sum_c m(f)[r][c] x[c][f]` for every line: a matrix stack times
/// a stack of vectors held as rows of spectra.
pub fn apply(m: &MatrixStack, x: &[Vec<C64>]) -> Vec<Vec<C64>> {
    let (lines, rows, cols) = m.dims();
    let mut out = vec![vec![C64::new(0.0, 0.0); lines]; rows];
    for f in 0..lines {
        let line = m.line(f);
        for r in 0..rows {
            let mut acc = C64::new(0.0, 0.0);
            for c in 0..cols {
                acc += line[r * cols + c] * x[c][f];
            }
            out[r][f] = acc;
        }
    }
    out
}

/// The rms of every row together.
pub fn rms_all(rows: &[Vec<f64>]) -> f64 {
    let n: usize = rows.iter().map(|r| r.len()).sum();
    if n == 0 {
        return 0.0;
    }
    let e: f64 = rows.iter().flat_map(|r| r.iter()).map(|v| v * v).sum();
    (e / n as f64).sqrt()
}

/// `norm(a - b) / norm(b)` over every row together.
pub fn relative_error(a: &[Vec<f64>], b: &[Vec<f64>]) -> f64 {
    let mut num = 0.0;
    let mut den = 0.0;
    for (ra, rb) in a.iter().zip(b) {
        for (x, y) in ra.iter().zip(rb) {
            num += (x - y) * (x - y);
            den += y * y;
        }
    }
    if den > 0.0 {
        (num / den).sqrt()
    } else {
        num.sqrt()
    }
}

/// Moving average of `x` over `width` lines (odd), treating lines past either
/// end as zero and always dividing by `width`, as a centered convolution does.
pub fn smooth(x: &[f64], width: usize) -> Vec<f64> {
    let w = width.max(1);
    let half = w / 2;
    let n = x.len();
    let mut prefix = vec![0.0; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + x[i];
    }
    (0..n)
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + w - half).min(n);
            (prefix[hi] - prefix[lo]) / w as f64
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_shock_spectrum_round_trip_and_sign() {
        let n = 64;
        // A cosine at line 3 delayed by one sample: phase exp(-j w dt).
        let x: Vec<f64> = (0..n)
            .map(|i| (std::f64::consts::TAU * 3.0 * (i as f64 - 1.0) / n as f64).cos())
            .collect();
        let s = rfft(&x).unwrap();
        let ph = s[3].arg();
        let want = -std::f64::consts::TAU * 3.0 / n as f64;
        assert!((ph - want).abs() < 1e-12, "{ph} {want}");
        let back = irfft(&s, n).unwrap();
        for (a, b) in back.iter().zip(&x) {
            assert!((a - b).abs() < 1e-12);
        }
    }

    #[test]
    fn mimo_shock_smooth_matches_centered_convolution() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        let s = smooth(&x, 3);
        let want = [1.0, 2.0, 3.0, 4.0, 3.0];
        for (a, b) in s.iter().zip(want) {
            assert!((a - b).abs() < 1e-12, "{s:?}");
        }
    }
}
