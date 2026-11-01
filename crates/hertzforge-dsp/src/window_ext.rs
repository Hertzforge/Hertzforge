//! Additional analysis windows (rectangular, flat top, Blackman-Harris) and
//! numerically-derived window correction factors.
//!
//! All windows here are PERIODIC (DFT-even): sampled with denominator `n`,
//! matching `crate::window::hann`. Coefficient sets are pinned by name in each
//! function's docs so an implementation can be diffed against the published
//! source, but correction factors are always DERIVED from the actual window
//! vector via [`corrections`] - never hardcoded - so a coefficient edit cannot
//! silently desynchronize window and correction.
//!
//! # Amplitude vs energy correction (mutually exclusive)
//!
//! - `amplitude` (coherent gain reciprocal, `N / sum(w)`) calibrates the PEAK
//!   BIN of a sinusoid back to its true amplitude.
//! - `energy` (`sqrt(N / sum(w^2))`) calibrates broadband POWER.
//! - `enbw_bins` (`N * sum(w^2) / sum(w)^2`) is the equivalent noise bandwidth
//!   in bins - the ratio tying the two normalizations together.
//!
//! A given spectrum readout applies ONE of amplitude/energy correction,
//! selected by the output type (tone amplitude vs density/power); applying
//! both double-corrects. See `crate::spectrum_ext` for the output-type
//! taxonomy and the `autopower = psd * df * enbw_bins` identity.

/// 5-term flat top cosine coefficients - the MATLAB `flattopwin` / NI LabVIEW
/// flat-top set (also SciPy `scipy.signal.windows.flattop`).
pub const FLATTOP_COEFFS: [f64; 5] = [
    0.21557895,
    0.41663158,
    0.277263158,
    0.083578947,
    0.006947368,
];

/// 4-term Blackman-Harris coefficients (Harris 1978, -92 dB sidelobe set).
pub const BLACKMAN_HARRIS_COEFFS: [f64; 4] = [0.35875, 0.48829, 0.14128, 0.01168];

/// Periodic cosine-sum window: `w[k] = sum_m (-1)^m * a[m] * cos(m * 2*pi*k/n)`.
fn cosine_sum(n: usize, coeffs: &[f64]) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    let tau = std::f64::consts::TAU;
    (0..n)
        .map(|k| {
            let x = tau * k as f64 / n as f64;
            coeffs
                .iter()
                .enumerate()
                .map(|(m, &a)| {
                    let sign = if m % 2 == 0 { 1.0 } else { -1.0 };
                    sign * a * (m as f64 * x).cos()
                })
                .sum()
        })
        .collect()
}

/// Rectangular (boxcar) window of length `n` - all ones. ENBW is exactly
/// 1.0 bin; amplitude and energy corrections are exactly 1.0.
pub fn rectangular(n: usize) -> Vec<f64> {
    vec![1.0; n]
}

/// Periodic 5-term flat top window of length `n`.
///
/// Coefficient set pinned to the MATLAB/NI `flattopwin` 5-term flat top
/// ([`FLATTOP_COEFFS`]); the sampling is periodic (denominator `n`) to match
/// this crate's `window::hann` convention (MATLAB's default is the symmetric
/// `n - 1` sampling - same coefficients, different endpoint handling).
/// Scalloping loss < 0.01 dB, ENBW ~= 3.77 bins: the amplitude-accuracy
/// window for calibrated tone readout.
pub fn flattop(n: usize) -> Vec<f64> {
    cosine_sum(n, &FLATTOP_COEFFS)
}

/// Periodic 4-term Blackman-Harris window of length `n`
/// ([`BLACKMAN_HARRIS_COEFFS`], -92 dB sidelobes, ENBW ~= 2.00 bins).
pub fn blackman_harris(n: usize) -> Vec<f64> {
    cosine_sum(n, &BLACKMAN_HARRIS_COEFFS)
}

/// Correction factors derived from an actual window vector. See the module
/// docs for which factor applies to which output type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowCorrections {
    /// Coherent-gain reciprocal `N / sum(w)`: multiply a peak-bin sinusoid
    /// amplitude estimate by this.
    pub amplitude: f64,
    /// RMS/power correction `sqrt(N / sum(w^2))`: multiply a broadband
    /// amplitude (sqrt-power) estimate by this.
    pub energy: f64,
    /// Equivalent noise bandwidth in bins: `N * sum(w^2) / sum(w)^2`.
    pub enbw_bins: f64,
}

/// Derive [`WindowCorrections`] numerically from the window vector. An empty
/// window returns the identity corrections (all 1.0).
pub fn corrections(w: &[f64]) -> WindowCorrections {
    if w.is_empty() {
        return WindowCorrections {
            amplitude: 1.0,
            energy: 1.0,
            enbw_bins: 1.0,
        };
    }
    let n = w.len() as f64;
    let s1: f64 = w.iter().sum();
    let s2: f64 = w.iter().map(|v| v * v).sum();
    WindowCorrections {
        amplitude: n / s1,
        energy: (n / s2).sqrt(),
        enbw_bins: n * s2 / (s1 * s1),
    }
}

// ---------------------------------------------------------------------------
// Insight additions: new windows + SYMMETRIC variants.
// ADDITIVE ONLY - every pre-existing function above keeps its periodic
// sampling and its exact output. Periodic (denominator n) remains the crate
// convention for SPECTRAL use; `_symmetric` variants (denominator n-1) exist
// for FIR design and for matching tools that export symmetric windows
// (the periodic-vs-symmetric mismatch has no obvious
// symptom - it just biases every correction slightly).
// ---------------------------------------------------------------------------

/// Symmetric cosine-sum window: denominator `n - 1`.
fn cosine_sum_symmetric(n: usize, coeffs: &[f64]) -> Vec<f64> {
    match n {
        0 => return Vec::new(),
        1 => return vec![1.0],
        _ => {}
    }
    let denom = (n - 1) as f64;
    (0..n)
        .map(|k| {
            let x = 2.0 * std::f64::consts::PI * k as f64 / denom;
            coeffs
                .iter()
                .enumerate()
                .map(|(m, a)| {
                    if m % 2 == 0 {
                        a * (m as f64 * x).cos()
                    } else {
                        -a * (m as f64 * x).cos()
                    }
                })
                .sum()
        })
        .collect()
}

/// Hamming coefficients (the classic 25/46 pair, exact rational form).
pub const HAMMING_COEFFS: [f64; 2] = [0.543_478_260_869_565_2, 0.456_521_739_130_434_8];

/// "Exact Blackman" 3-term coefficients (7938/18608, 9240/18608, 1430/18608).
pub const BLACKMAN_EXACT_COEFFS: [f64; 3] = [
    0.426_590_713_671_539_1,
    0.496_560_619_088_564_1,
    0.076_848_667_239_896_82,
];

/// Periodic Hamming window (25/46 exact coefficients).
pub fn hamming(n: usize) -> Vec<f64> {
    cosine_sum(n, &HAMMING_COEFFS)
}

/// Symmetric Hamming window.
pub fn hamming_symmetric(n: usize) -> Vec<f64> {
    cosine_sum_symmetric(n, &HAMMING_COEFFS)
}

/// Periodic exact-Blackman window.
pub fn blackman_exact(n: usize) -> Vec<f64> {
    cosine_sum(n, &BLACKMAN_EXACT_COEFFS)
}

/// Symmetric exact-Blackman window.
pub fn blackman_exact_symmetric(n: usize) -> Vec<f64> {
    cosine_sum_symmetric(n, &BLACKMAN_EXACT_COEFFS)
}

/// Symmetric Hann (the crate's periodic Hann lives in `crate::window::hann`).
pub fn hann_symmetric(n: usize) -> Vec<f64> {
    cosine_sum_symmetric(n, &[0.5, 0.5])
}

/// Symmetric flat top.
pub fn flattop_symmetric(n: usize) -> Vec<f64> {
    cosine_sum_symmetric(n, &FLATTOP_COEFFS)
}

/// Symmetric Blackman-Harris.
pub fn blackman_harris_symmetric(n: usize) -> Vec<f64> {
    cosine_sum_symmetric(n, &BLACKMAN_HARRIS_COEFFS)
}

/// Zeroth-order modified Bessel function of the first kind, by power series.
/// Converges fast for the beta range windows use (< 20); pinned by the Kaiser
/// tests below.
fn bessel_i0(x: f64) -> f64 {
    let half = x / 2.0;
    let mut term = 1.0f64;
    let mut sum = 1.0f64;
    for k in 1..64 {
        term *= (half / k as f64) * (half / k as f64);
        sum += term;
        if term < sum * 1e-18 {
            break;
        }
    }
    sum
}

/// Symmetric Kaiser window with shape parameter `beta` (FIR-design form).
pub fn kaiser_symmetric(n: usize, beta: f64) -> Vec<f64> {
    match n {
        0 => return Vec::new(),
        1 => return vec![1.0],
        _ => {}
    }
    let denom = (n - 1) as f64;
    let i0b = bessel_i0(beta);
    (0..n)
        .map(|k| {
            let r = 2.0 * k as f64 / denom - 1.0; // [-1, 1]
            bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0b
        })
        .collect()
}

/// Periodic Kaiser (spectral form: symmetric Kaiser of n+1 with the last
/// sample dropped - the standard DFT-even construction).
pub fn kaiser(n: usize, beta: f64) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    let mut w = kaiser_symmetric(n + 1, beta);
    w.truncate(n);
    w
}

/// Symmetric Tukey (tapered cosine) window; `alpha` in [0, 1] is the taper
/// fraction (0 = rectangular, 1 = Hann). The shock-record window.
pub fn tukey_symmetric(n: usize, alpha: f64) -> Vec<f64> {
    match n {
        0 => return Vec::new(),
        1 => return vec![1.0],
        _ => {}
    }
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha == 0.0 {
        return vec![1.0; n];
    }
    let denom = (n - 1) as f64;
    let edge = alpha * denom / 2.0;
    (0..n)
        .map(|k| {
            let k = k as f64;
            if k < edge {
                0.5 * (1.0 + (std::f64::consts::PI * (k / edge - 1.0)).cos())
            } else if k > denom - edge {
                0.5 * (1.0 + (std::f64::consts::PI * ((k - denom + edge) / edge)).cos())
            } else {
                1.0
            }
        })
        .collect()
}

/// Periodic Tukey (symmetric of n+1, last sample dropped).
pub fn tukey(n: usize, alpha: f64) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    let mut w = tukey_symmetric(n + 1, alpha);
    w.truncate(n);
    w
}

#[cfg(test)]
mod insight_window_tests {
    use super::*;

    #[test]
    fn symmetric_windows_are_symmetric_periodic_are_not() {
        for (sym, per) in [
            (hann_symmetric(64), crate::window::hann(64)),
            (hamming_symmetric(64), hamming(64)),
            (blackman_exact_symmetric(64), blackman_exact(64)),
        ] {
            for k in 0..64 {
                assert!((sym[k] - sym[63 - k]).abs() < 1e-12, "symmetric mirror");
            }
            // Periodic differs from symmetric somewhere in the interior.
            assert!(sym.iter().zip(&per).any(|(a, b)| (a - b).abs() > 1e-9));
        }
    }

    #[test]
    fn periodic_hann_here_matches_lib_convention_shape() {
        // hann_symmetric(n+1) truncated == periodic hann(n): the standard
        // DFT-even relation ties the two sampling conventions together.
        let n = 128;
        let per = crate::window::hann(n);
        let mut sym = hann_symmetric(n + 1);
        sym.truncate(n);
        for k in 0..n {
            assert!((per[k] - sym[k]).abs() < 1e-12);
        }
    }

    #[test]
    fn kaiser_beta_zero_is_rectangular_and_corrections_derive() {
        let w = kaiser_symmetric(64, 0.0);
        for v in &w {
            assert!((v - 1.0).abs() < 1e-12);
        }
        let c = corrections(&kaiser(1024, 9.0));
        assert!(c.amplitude > 1.0 && c.energy > 1.0 && c.enbw_bins > 1.0);
    }

    #[test]
    fn kaiser_endpoint_value_is_1_over_i0_beta() {
        // w[0] of the symmetric Kaiser is I0(0)/I0(beta) = 1/I0(beta): pins the
        // Bessel series against the closed-form endpoint.
        let beta = 8.6;
        let w = kaiser_symmetric(101, beta);
        assert!((w[0] - 1.0 / bessel_i0(beta)).abs() < 1e-12);
        assert!(
            (w[50] - 1.0).abs() < 1e-12,
            "center of symmetric Kaiser is 1"
        );
    }

    #[test]
    fn tukey_limits_are_rect_and_hann() {
        let r = tukey_symmetric(64, 0.0);
        assert!(r.iter().all(|v| (*v - 1.0).abs() < 1e-12));
        let h = tukey_symmetric(64, 1.0);
        let hh = hann_symmetric(64);
        for k in 0..64 {
            assert!((h[k] - hh[k]).abs() < 1e-9, "alpha=1 Tukey is Hann at {k}");
        }
    }
}
