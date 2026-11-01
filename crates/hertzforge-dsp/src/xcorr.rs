//! Cross-correlation for Insight transforms. FFT method on
//! the crate's own radix-2 kernels. New-files-only rule.
//!
//! LAG CONVENTION (pinned by test, stated verbatim in every recipe):
//! `r[m] = sum_n b[n] * a[n - m]` - a POSITIVE lag means b lags a.

use crate::fft;

/// Normalization of the correlation values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum XcorrNorm {
    /// Raw sums.
    Raw,
    /// Divided by max(len_a, len_b).
    Biased,
    /// Correlation coefficient: divided by sqrt(energy_a * energy_b), so
    /// |r| <= 1 and identical signals peak at exactly 1.
    Coeff,
}

/// Cross-correlation result: values over lags
/// `-(len_a - 1) ..= (len_b - 1)`, in samples of the shared rate.
#[derive(Debug, Clone)]
pub struct Xcorr {
    pub values: Vec<f64>,
    /// Lag of `values[0]` in samples (negative).
    pub first_lag: i64,
}

/// Cross-correlate `b` against `a` (see the module lag convention). The two
/// signals must share a sample rate - the CALLER enforces and discloses that.
pub fn xcorr(a: &[f64], b: &[f64], norm: XcorrNorm) -> Result<Xcorr, String> {
    if a.len() < 2 || b.len() < 2 {
        return Err("cross-correlation needs at least 2 samples per signal".into());
    }
    let na = a.len();
    let nb = b.len();
    let nfft = fft::next_pow2(na + nb - 1);
    let mut fa = vec![(0.0, 0.0); nfft];
    for (i, &v) in a.iter().enumerate() {
        fa[i] = (v, 0.0);
    }
    let mut fb = vec![(0.0, 0.0); nfft];
    for (i, &v) in b.iter().enumerate() {
        fb[i] = (v, 0.0);
    }
    let sa = fft::fft(&fa);
    let sb = fft::fft(&fb);
    // R = IFFT( B .* conj(A) ): circular r[m] = sum b[n] a[n-m], negative
    // lags wrapped to the top of the buffer.
    let prod: Vec<fft::Cplx> = sa
        .iter()
        .zip(&sb)
        .map(|(&(ar, ai), &(br, bi))| (br * ar + bi * ai, bi * ar - br * ai))
        .collect();
    let r = fft::ifft(&prod);
    let first_lag = -(na as i64 - 1);
    let n_lags = na + nb - 1;
    let mut values = Vec::with_capacity(n_lags);
    for k in 0..n_lags {
        let m = first_lag + k as i64;
        let idx = if m < 0 { nfft as i64 + m } else { m } as usize;
        values.push(r[idx].0);
    }
    let scale = match norm {
        XcorrNorm::Raw => 1.0,
        XcorrNorm::Biased => 1.0 / na.max(nb) as f64,
        XcorrNorm::Coeff => {
            let ea: f64 = a.iter().map(|v| v * v).sum();
            let eb: f64 = b.iter().map(|v| v * v).sum();
            let d = (ea * eb).sqrt();
            if d > 0.0 {
                1.0 / d
            } else {
                return Err("cross-correlation of an all-zero signal".into());
            }
        }
    };
    if scale != 1.0 {
        for v in values.iter_mut() {
            *v *= scale;
        }
    }
    Ok(Xcorr { values, first_lag })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize) -> Vec<f64> {
        // Deterministic pseudo-noise (LCG) - tests must not use RNG state.
        let mut s = 0x2545F4914F6CDD1Du64;
        (0..n)
            .map(|_| {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((s >> 33) as f64 / (1u64 << 31) as f64) - 1.0
            })
            .collect()
    }

    #[test]
    fn a_delayed_copy_peaks_at_positive_lag_with_coeff_near_one() {
        let a = noise(4096);
        let d = 10usize;
        // b[n] = a[n - d]: b lags a by d.
        let mut b = vec![0.0; a.len()];
        b[d..].copy_from_slice(&a[..a.len() - d]);
        let r = xcorr(&a, &b, XcorrNorm::Coeff).unwrap();
        let (imax, vmax) = r
            .values
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.partial_cmp(y.1).unwrap())
            .unwrap();
        assert_eq!(r.first_lag + imax as i64, d as i64, "peak lag");
        assert!(*vmax > 0.95, "coeff peak {vmax}");
        for v in &r.values {
            assert!(*v <= 1.0 + 1e-9, "coeff bound violated: {v}");
        }
    }

    #[test]
    fn autocorrelation_peaks_at_lag_zero_with_exactly_one() {
        let a = noise(2048);
        let r = xcorr(&a, &a, XcorrNorm::Coeff).unwrap();
        let mid = (-r.first_lag) as usize;
        assert!((r.values[mid] - 1.0).abs() < 1e-9);
        for (k, v) in r.values.iter().enumerate() {
            if k != mid {
                assert!(*v < r.values[mid]);
            }
        }
    }

    #[test]
    fn raw_matches_the_direct_sum_on_a_small_case() {
        let a = [1.0, 2.0, 3.0];
        let b = [4.0, 5.0, 6.0];
        let r = xcorr(&a, &b, XcorrNorm::Raw).unwrap();
        // r[m] = sum b[n] a[n-m], lags -2..=2:
        // m=-2: b[0]a[2]=12; m=-1: b[0]a[1]+b[1]a[2]=8+15=23;
        // m=0: 4+10+18=32; m=1: b[1]a[0]+b[2]a[1]=5+12=17; m=2: b[2]a[0]=6.
        let expect = [12.0, 23.0, 32.0, 17.0, 6.0];
        assert_eq!(r.first_lag, -2);
        for (got, want) in r.values.iter().zip(expect) {
            assert!((got - want).abs() < 1e-9, "{got} vs {want}");
        }
    }

    #[test]
    fn degenerate_inputs_are_refused() {
        assert!(xcorr(&[1.0], &[1.0, 2.0], XcorrNorm::Raw).is_err());
        assert!(xcorr(&[0.0, 0.0], &[1.0, 2.0], XcorrNorm::Coeff).is_err());
    }
}
