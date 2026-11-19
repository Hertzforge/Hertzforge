//! Offline (batch) spectral estimation with caller-supplied windows - the
//! Insight reprocessing kernels.
//!
//! ADDITIVE: the live control path's `spectrum::psd_welch` (Hann, per-segment
//! mean detrend, hardcoded) is untouched; `psd_welch_with(..)` generalizes it
//! and the equivalence test below pins that
//! `psd_welch_with(hann, SegmentMean) == psd_welch` bit-for-bit-close.
//!
//! Normalization taxonomy (matches `spectrum_ext`):
//! - PSD density: `1/(fs * S2)`, S2 = sum(w^2), one-sided fold doubles
//!   interior bins, never DC or Nyquist, applied ONCE (Parseval fence).
//! - Autopower (EU^2 rms): `1/S1^2`, S1 = sum(w).
//! - Linear spectrum (EU rms): sqrt(autopower).

use crate::fft;

/// Kernel-level detrend policy (a policy ENUM at kernel granularity is fine;
/// deciding WHICH to use per function kind is the module crate's job).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detrend {
    None,
    SegmentMean,
}

/// Averaged one-sided spectra from overlapped windowed segments.
#[derive(Debug, Clone)]
pub struct WelchOut {
    /// Bin frequencies 0..=fs/2 (nfft/2 + 1 bins).
    pub freqs: Vec<f64>,
    /// The averaged quantity (see the producing function for its scaling).
    pub values: Vec<f64>,
    /// Number of segments averaged. 0 means "signal shorter than one segment"
    /// and empty vectors.
    pub segments: usize,
}

fn validate(window: &[f64], overlap: f64, fs_hz: f64) -> Result<(usize, usize), String> {
    let nfft = window.len();
    if nfft < 2 || !nfft.is_power_of_two() {
        return Err(format!(
            "window length must be a power of two >= 2 (got {nfft})"
        ));
    }
    if fs_hz <= 0.0 {
        return Err(format!("fs must be positive (got {fs_hz})"));
    }
    if !(0.0..=0.95).contains(&overlap) {
        return Err(format!("overlap must be in [0, 0.95] (got {overlap})"));
    }
    let step = ((nfft as f64) * (1.0 - overlap)).round().max(1.0) as usize;
    Ok((nfft, step))
}

fn segment_spectra<F: FnMut(usize, &[(f64, f64)])>(
    signal: &[f64],
    window: &[f64],
    step: usize,
    detrend: Detrend,
    mut per_segment: F,
) -> usize {
    let nfft = window.len();
    let mut seg = vec![(0.0f64, 0.0f64); nfft];
    let mut nseg = 0usize;
    let mut start = 0usize;
    while start + nfft <= signal.len() {
        let mean = match detrend {
            Detrend::None => 0.0,
            Detrend::SegmentMean => signal[start..start + nfft].iter().sum::<f64>() / nfft as f64,
        };
        for n in 0..nfft {
            seg[n] = ((signal[start + n] - mean) * window[n], 0.0);
        }
        fft::fft_in_place(&mut seg);
        per_segment(nseg, &seg);
        nseg += 1;
        start += step;
    }
    nseg
}

/// One-sided PSD (EU^2/Hz) with a caller-supplied window - `psd_welch`
/// generalized. Returns Err on invalid parameters so the kernel assert in the
/// live path is unreachable from Insight.
pub fn psd_welch_with(
    signal: &[f64],
    fs_hz: f64,
    window: &[f64],
    overlap: f64,
    detrend: Detrend,
) -> Result<WelchOut, String> {
    let (nfft, step) = validate(window, overlap, fs_hz)?;
    let s2: f64 = window.iter().map(|v| v * v).sum();
    if s2 == 0.0 {
        return Err("window is all zeros".into());
    }
    let norm = 1.0 / (fs_hz * s2);
    let nout = nfft / 2 + 1;
    let mut acc = vec![0.0f64; nout];
    let nseg = segment_spectra(signal, window, step, detrend, |_, seg| {
        for (k, a) in acc.iter_mut().enumerate() {
            let (re, im) = seg[k];
            let mut p = (re * re + im * im) * norm;
            let is_nyquist = k == nfft / 2;
            if k != 0 && !is_nyquist {
                p *= 2.0;
            }
            *a += p;
        }
    });
    if nseg == 0 {
        return Ok(WelchOut {
            freqs: Vec::new(),
            values: Vec::new(),
            segments: 0,
        });
    }
    let inv = 1.0 / nseg as f64;
    for a in acc.iter_mut() {
        *a *= inv;
    }
    let df = fs_hz / nfft as f64;
    Ok(WelchOut {
        freqs: (0..nout).map(|k| k as f64 * df).collect(),
        values: acc,
        segments: nseg,
    })
}

/// One-sided AUTOPOWER (EU^2 rms per bin): `|X|^2 / S1^2`, interior bins
/// doubled. `sqrt(peak bin)` is the rms of a tone regardless of window
/// (amplitude-corrected by construction - `spectrum_ext` taxonomy).
pub fn autopower_welch_with(
    signal: &[f64],
    fs_hz: f64,
    window: &[f64],
    overlap: f64,
    detrend: Detrend,
) -> Result<WelchOut, String> {
    let (nfft, step) = validate(window, overlap, fs_hz)?;
    let s1: f64 = window.iter().sum();
    if s1 == 0.0 {
        return Err("window sums to zero".into());
    }
    let norm = 1.0 / (s1 * s1);
    let nout = nfft / 2 + 1;
    let mut acc = vec![0.0f64; nout];
    let nseg = segment_spectra(signal, window, step, detrend, |_, seg| {
        for (k, a) in acc.iter_mut().enumerate() {
            let (re, im) = seg[k];
            let mut p = (re * re + im * im) * norm;
            let is_nyquist = k == nfft / 2;
            if k != 0 && !is_nyquist {
                p *= 2.0;
            }
            *a += p;
        }
    });
    if nseg == 0 {
        return Ok(WelchOut {
            freqs: Vec::new(),
            values: Vec::new(),
            segments: 0,
        });
    }
    let inv = 1.0 / nseg as f64;
    for a in acc.iter_mut() {
        *a *= inv;
    }
    let df = fs_hz / nfft as f64;
    Ok(WelchOut {
        freqs: (0..nout).map(|k| k as f64 * df).collect(),
        values: acc,
        segments: nseg,
    })
}

/// Averaged one-sided LINEAR SPECTRUM (EU rms per bin): sqrt(autopower).
pub fn linear_spectrum_with(
    signal: &[f64],
    fs_hz: f64,
    window: &[f64],
    overlap: f64,
    detrend: Detrend,
) -> Result<WelchOut, String> {
    let mut out = autopower_welch_with(signal, fs_hz, window, overlap, detrend)?;
    for v in out.values.iter_mut() {
        *v = v.sqrt();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum;
    use crate::window;

    fn test_signal(n: usize, fs: f64) -> Vec<f64> {
        // Deterministic multi-tone + pseudo-noise + a DC offset (for the
        // detrend and Parseval fences).
        let mut lcg: u64 = 0x2545F4914F6CDD1D;
        (0..n)
            .map(|i| {
                lcg = lcg
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let noise = ((lcg >> 33) as f64 / (1u64 << 31) as f64) - 1.0;
                let t = i as f64 / fs;
                0.7 + 1.4 * (2.0 * std::f64::consts::PI * 137.0 * t).sin()
                    + 0.6 * (2.0 * std::f64::consts::PI * 411.0 * t).sin()
                    + 0.25 * noise
            })
            .collect()
    }

    #[test]
    fn equivalence_with_live_psd_welch_hann_segmentmean() {
        // The equivalence fence: the generalized kernel with Hann + SegmentMean
        // must reproduce the live path bit-for-bit-close.
        let fs = 8192.0;
        let x = test_signal(1 << 15, fs);
        let nfft = 2048;
        let (f0, p0) = spectrum::psd_welch(&x, fs, nfft, 0.5);
        let w = window::hann(nfft);
        let out = psd_welch_with(&x, fs, &w, 0.5, Detrend::SegmentMean).unwrap();
        assert_eq!(f0.len(), out.freqs.len());
        for (k, &pk) in p0.iter().enumerate() {
            let d = (pk - out.values[k]).abs();
            let scale = pk.abs().max(1e-30);
            assert!(
                d / scale < 1e-12,
                "bin {k}: live {} vs offline {}",
                pk,
                out.values[k]
            );
        }
    }

    #[test]
    fn parseval_single_window_correction_with_dc_offset() {
        // Rectangular window, Detrend::None, DC offset present: the integral of
        // the PSD over [0, fs/2] must equal the mean-square of the signal - a
        // double-applied window correction or a doubled DC bin breaks this
        // (DC bin owns [0, df/2]).
        let fs = 4096.0;
        let nfft = 4096usize;
        let x = test_signal(nfft, fs); // exactly one segment
        let w = vec![1.0; nfft];
        let out = psd_welch_with(&x, fs, &w, 0.0, Detrend::None).unwrap();
        assert_eq!(out.segments, 1);
        let df = fs / nfft as f64;
        // Integrate with the DC/Nyquist edge convention: every bin contributes
        // psd*df; DC and Nyquist carry half-width bins BUT are also not doubled
        // in the one-sided fold, so plain psd*df over all bins is exact for the
        // rectangular single-segment case.
        let integral: f64 = out.values.iter().map(|p| p * df).sum();
        let msq = x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64;
        assert!(
            ((integral - msq) / msq).abs() < 1e-9,
            "Parseval: integral {integral} vs mean-square {msq}"
        );
    }

    #[test]
    fn autopower_peak_reads_tone_rms_regardless_of_window() {
        let fs = 8192.0;
        let nfft = 8192usize;
        let a = 2.0_f64; // peak amplitude -> rms a/sqrt(2)

        // Bin-centered tone so leakage does not split the line.
        let f0 = 128.0 * fs / nfft as f64;
        let x: Vec<f64> = (0..nfft * 4)
            .map(|i| a * (2.0 * std::f64::consts::PI * f0 * i as f64 / fs).sin())
            .collect();
        for w in [window::hann(nfft), vec![1.0; nfft]] {
            let out = autopower_welch_with(&x, fs, &w, 0.0, Detrend::None).unwrap();
            let peak = out.values.iter().cloned().fold(0.0f64, f64::max);
            let rms = peak.sqrt();
            assert!(
                (rms - a / std::f64::consts::SQRT_2).abs() < 1e-6,
                "tone rms via autopower: {rms}"
            );
        }
    }

    #[test]
    fn invalid_params_error_do_not_panic() {
        let x = vec![0.0; 4096];
        assert!(psd_welch_with(&x, 1024.0, &[1.0; 1000], 0.5, Detrend::None).is_err());
        assert!(psd_welch_with(&x, -1.0, &[1.0; 1024], 0.5, Detrend::None).is_err());
        assert!(psd_welch_with(&x, 1024.0, &[1.0; 1024], 0.99, Detrend::None).is_err());
        assert!(psd_welch_with(&x, 1024.0, &[0.0; 1024], 0.5, Detrend::None).is_err());
    }

    #[test]
    fn short_signal_reports_zero_segments() {
        let out = psd_welch_with(&[1.0; 100], 1024.0, &[1.0; 1024], 0.5, Detrend::None).unwrap();
        assert_eq!(out.segments, 0);
        assert!(out.values.is_empty());
    }
}
