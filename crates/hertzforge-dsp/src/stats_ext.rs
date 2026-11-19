//! Offline statistics for Insight.
//!
//! Additive to `stats` in lib.rs - the live path's `rms` stays DC-inclusive;
//! AC-coupled and higher-moment statistics live here.

use crate::regrid;
use crate::stats;

/// AC-coupled RMS: mean removed first (added ALONGSIDE `stats::rms`).
pub fn rms_ac(x: &[f64]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    let m = stats::mean(x);
    (x.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / x.len() as f64).sqrt()
}

/// Population standard deviation (= rms_ac; named for discoverability).
pub fn std_dev(x: &[f64]) -> f64 {
    rms_ac(x)
}

/// Crest factor: |peak| / rms (DC-inclusive rms, matching how overload and
/// drive budgets are quoted). 0 for empty or silent records.
pub fn crest(x: &[f64]) -> f64 {
    let r = stats::rms(x);
    if r == 0.0 {
        return 0.0;
    }
    stats::peak(x) / r
}

/// Kurtosis as the normalized fourth moment beta2 = m4 / m2^2 (Gaussian = 3.0,
/// the vibration-community convention - NOT excess kurtosis). Computed about
/// the mean. Returns 0 for records shorter than 2 samples or zero variance.
pub fn kurtosis(x: &[f64]) -> f64 {
    if x.len() < 2 {
        return 0.0;
    }
    let m = stats::mean(x);
    let n = x.len() as f64;
    let m2 = x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / n;
    if m2 == 0.0 {
        return 0.0;
    }
    let m4 = x.iter().map(|v| (v - m).powi(4)).sum::<f64>() / n;
    m4 / (m2 * m2)
}

/// Skewness m3 / m2^(3/2). 0 for degenerate records.
pub fn skewness(x: &[f64]) -> f64 {
    if x.len() < 2 {
        return 0.0;
    }
    let m = stats::mean(x);
    let n = x.len() as f64;
    let m2 = x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / n;
    if m2 == 0.0 {
        return 0.0;
    }
    let m3 = x.iter().map(|v| (v - m).powi(3)).sum::<f64>() / n;
    m3 / m2.powf(1.5)
}

/// Running RMS over a sliding window of `win` samples (DC-inclusive), output
/// aligned to the window END (causal). Output length = `x.len()`; the first
/// `win-1` samples use the partial window (honest start-up, no zero padding).
pub fn running_rms(x: &[f64], win: usize) -> Vec<f64> {
    let win = win.max(1);
    let mut out = Vec::with_capacity(x.len());
    let mut acc = 0.0f64;
    for i in 0..x.len() {
        acc += x[i] * x[i];
        if i >= win {
            acc -= x[i - win] * x[i - win];
            // Guard tiny negative residue from cancellation.
            if acc < 0.0 {
                acc = 0.0;
            }
        }
        let n = (i + 1).min(win) as f64;
        out.push((acc / n).sqrt());
    }
    out
}

/// Band RMS from a PSD: sqrt of the integral of the density over `[f_lo, f_hi]`
/// using the log-log power-law segment model (the gRMS number every operator
/// asks for first - incl. the log-log trap: trapezoid on
/// log-displayed data misintegrates; the power-law model is the correct read).
///
/// `centers`/`psd` with `edges` (len+1) as produced by `Abscissa::edges()`.
/// Returns `None` when the band does not intersect the source span.
pub fn band_rms_psd(
    centers: &[f64],
    psd: &[f64],
    edges: &[f64],
    f_lo: f64,
    f_hi: f64,
) -> Option<f64> {
    if centers.is_empty() || edges.len() != centers.len() + 1 {
        return None;
    }
    let (lo, hi) = regrid::span_intersection((edges[0], edges[edges.len() - 1]), (f_lo, f_hi))?;
    let area = regrid::model_integral(centers, psd, edges, lo, hi);
    (area >= 0.0).then(|| area.sqrt())
}

/// A breakpoint of a log-log profile: (frequency Hz, density level EU^2/Hz).
pub type Breakpoint = (f64, f64);

/// RMS of a breakpoint PROFILE (the spec-sheet gRMS): integrate the log-log
/// straight-line segments between breakpoints analytically, with the n = -1
/// logarithmic branch guarded (the -3.01 dB/octave roll-off every random spec
/// uses lands exactly on the singularity).
/// Breakpoints must be strictly increasing in frequency, positive in both
/// coordinates.
pub fn profile_rms(bps: &[Breakpoint]) -> Result<f64, String> {
    if bps.len() < 2 {
        return Err("profile needs at least 2 breakpoints".into());
    }
    let mut area = 0.0;
    for w in bps.windows(2) {
        let ((f1, y1), (f2, y2)) = (w[0], w[1]);
        // Positive failure condition with explicit NaN guards, so a NaN frequency
        // is still refused (clippy neg_cmp_op_on_partial_ord).
        if f2 <= f1 || f1.is_nan() || f2.is_nan() {
            return Err(format!("breakpoints not increasing at {f1} -> {f2} Hz"));
        }
        if f1 <= 0.0 || y1 <= 0.0 || y2 <= 0.0 {
            return Err("breakpoints must be positive in f and level".into());
        }
        area += regrid::powerlaw_segment_integral(f1, y1, f2, y2, f1, f2);
    }
    Ok(area.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crest_of_a_sine_is_sqrt2() {
        let x: Vec<f64> = (0..4096).map(|i| (i as f64 * 0.1).sin()).collect();
        assert!((crest(&x) - std::f64::consts::SQRT_2).abs() < 0.01);
    }

    #[test]
    fn kurtosis_of_a_sine_is_1p5_and_convention_is_beta2() {
        let x: Vec<f64> = (0..65536).map(|i| (i as f64 * 0.01).sin()).collect();
        assert!(
            (kurtosis(&x) - 1.5).abs() < 0.01,
            "sine beta2 = 1.5, got {}",
            kurtosis(&x)
        );
    }

    #[test]
    fn rms_ac_removes_dc_while_stats_rms_keeps_it() {
        let x = vec![5.0; 1024];
        assert!((crate::stats::rms(&x) - 5.0).abs() < 1e-12);
        assert!(rms_ac(&x) < 1e-12);
    }

    #[test]
    fn running_rms_converges_to_block_rms() {
        let x: Vec<f64> = (0..4096).map(|i| (i as f64 * 0.37).sin()).collect();
        let rr = running_rms(&x, 1024);
        let tail = rr[rr.len() - 1];
        let block = crate::stats::rms(&x[x.len() - 1024..]);
        assert!((tail - block).abs() < 1e-9);
    }

    #[test]
    fn profile_rms_matches_the_flat_segment_hand_calc() {
        // Flat 0.04 g^2/Hz from 20 to 2000 Hz: gRMS = sqrt(0.04 * 1980) = 8.899...
        let g = profile_rms(&[(20.0, 0.04), (2000.0, 0.04)]).unwrap();
        assert!((g - (0.04f64 * 1980.0).sqrt()).abs() < 1e-9);
    }

    #[test]
    fn profile_rms_handles_the_minus_one_exponent_rolloff() {
        // A -3.0103 dB/octave segment is y ~ 1/f (n = -1): area = y1*f1*ln(f2/f1).
        let (f1, f2) = (100.0, 400.0);
        let y1 = 0.1;
        let y2 = y1 * f1 / f2; // exact 1/f
        let g = profile_rms(&[(f1, y1), (f2, y2)]).unwrap();
        let analytic = (y1 * f1 * (f2 / f1).ln()).sqrt();
        assert!((g - analytic).abs() / analytic < 1e-9);
    }

    #[test]
    fn band_rms_agrees_with_time_domain_on_white_noise_psd() {
        // Flat PSD level L over [0, fN]: band [0, fN] rms = sqrt(L * fN).
        let n = 512;
        let df = 2.0;
        let centers: Vec<f64> = (0..n).map(|i| df * i as f64).collect();
        let mut edges = vec![0.0];
        for w in centers.windows(2) {
            edges.push((w[0] + w[1]) / 2.0);
        }
        edges.push(centers[n - 1] + df / 2.0);
        let psd = vec![0.01; n];
        let full = band_rms_psd(&centers, &psd, &edges, 0.0, centers[n - 1] + df / 2.0).unwrap();
        let span = edges[n] - edges[0];
        assert!((full - (0.01f64 * span).sqrt()).abs() < 1e-9);
    }

    #[test]
    fn band_rms_refuses_disjoint_band() {
        let centers = [10.0, 20.0, 30.0];
        let psd = [1.0, 1.0, 1.0];
        let edges = [5.0, 15.0, 25.0, 35.0];
        assert!(band_rms_psd(&centers, &psd, &edges, 100.0, 200.0).is_none());
    }

    #[test]
    fn profile_rms_refuses_a_nan_frequency() {
        assert!(profile_rms(&[(20.0, 0.04), (f64::NAN, 0.04)]).is_err());
        assert!(profile_rms(&[(f64::NAN, 0.04), (2000.0, 0.04)]).is_err());
    }
}
