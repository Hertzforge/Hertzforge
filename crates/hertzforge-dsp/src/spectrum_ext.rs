//! Generalized single-block spectrum + output-type normalizations.
//!
//! `spectrum::psd_welch` stays UNTOUCHED as the Hann golden reference; this
//! module factors the same math into a window-generic single-block
//! [`periodogram`] plus separable normalizations, so the LIVE pipeline
//! (`periodogram` per block -> `crate::averaging::SpectrumAverager` ->
//! normalize) and the OFFLINE region path share one implementation. The
//! equivalence golden in `tests/` pins `avg(periodogram) + to_psd ==
//! psd_welch` on the same signal.
//!
//! # Output-type taxonomy (and which correction belongs to which)
//!
//! | Output            | Units    | Normalization from raw `|X|^2`            |
//! |-------------------|----------|-------------------------------------------|
//! | PSD (density)     | EU^2/Hz  | `1 / (fs * sum(w^2))`  (energy)            |
//! | Autopower         | EU^2 rms | `1 / sum(w)^2`         (amplitude)         |
//! | EUrms (tone)      | EU rms   | `sqrt(autopower_peak_bin)`                 |
//! | EUpk (tone)       | EU peak  | `sqrt(2 * autopower_peak_bin)`             |
//!
//! Amplitude (coherent-gain) and energy corrections are MUTUALLY EXCLUSIVE by
//! output type: the autopower normalization already contains the window's
//! amplitude correction (`sum(w)^2` in the denominator), and the PSD
//! normalization already contains the energy correction (`sum(w^2)`). Never
//! stack a `WindowCorrections` factor on top of these normalizations. The two
//! are tied together by the identity (with `df = fs/nfft` and `enbw_bins` from
//! `crate::window_ext::corrections`):
//!
//! ```text
//!   autopower[k] == psd[k] * df * enbw_bins
//! ```
//!
//! Reading rule: a discrete TONE is read from the autopower PEAK BIN
//! (leakage-summing is not required for the amplitude - the coherent gain
//! normalization makes the peak bin read `A^2/2` for an on-bin tone);
//! broadband/random content is read from the PSD.

use crate::fft;

/// Raw single-sided periodogram of one segment: per-segment mean removed,
/// windowed, FFT'd, `|X[k]|^2` with interior bins doubled (DC and Nyquist have
/// no negative-frequency twin and are NOT doubled). NO normalization is
/// applied - feed the result to [`to_psd`] or [`to_autopower`] (directly or
/// after averaging raw periodograms; the normalizations are linear so the
/// order does not matter).
///
/// `_fs_hz` is unused today (normalization lives in the `to_*` helpers) but
/// kept in the signature so call sites carry the acquisition context.
///
/// Panics unless `seg.len() == window.len()` and the length is a power of two
/// `>= 2` (same radix-2 constraint as `spectrum::psd_welch`). Returns
/// `nfft/2 + 1` bins.
pub fn periodogram(seg: &[f64], window: &[f64], _fs_hz: f64) -> Vec<f64> {
    let nfft = seg.len();
    assert_eq!(
        nfft,
        window.len(),
        "periodogram: segment and window lengths must match"
    );
    assert!(
        nfft >= 2 && nfft.is_power_of_two(),
        "periodogram: length must be a power of two >= 2 (got {nfft})"
    );
    let mean = seg.iter().sum::<f64>() / nfft as f64;
    let mut buf: Vec<fft::Cplx> = seg
        .iter()
        .zip(window)
        .map(|(&x, &w)| ((x - mean) * w, 0.0))
        .collect();
    fft::fft_in_place(&mut buf);

    let nout = nfft / 2 + 1;
    (0..nout)
        .map(|k| {
            let (re, im) = buf[k];
            let mut p = re * re + im * im;
            let is_nyquist = k == nfft / 2;
            if k != 0 && !is_nyquist {
                p *= 2.0;
            }
            p
        })
        .collect()
}

/// Normalize a raw periodogram (or an average of raw periodograms) to a
/// one-sided power spectral DENSITY in EU^2/Hz: divide by `fs * sum(w^2)`.
/// This is exactly `spectrum::psd_welch`'s convention, so
/// `integral(psd) df == variance`.
///
/// Panics if the window is empty or `fs_hz` is not positive.
pub fn to_psd(raw: &mut [f64], window: &[f64], fs_hz: f64) {
    assert!(fs_hz > 0.0, "to_psd: fs_hz must be positive");
    let s2: f64 = window.iter().map(|v| v * v).sum();
    assert!(s2 > 0.0, "to_psd: window power must be positive");
    let norm = 1.0 / (fs_hz * s2);
    for p in raw.iter_mut() {
        *p *= norm;
    }
}

/// Normalize a raw periodogram (or an average of raw periodograms) to
/// AUTOPOWER in EU^2 rms per bin: divide by `sum(w)^2`.
///
/// This carries the window's coherent-gain (amplitude) correction, so an
/// on-bin tone of peak amplitude `A` reads `A^2/2` (= EU^2 rms) in its peak
/// bin under ANY window, and the identity `autopower = psd * df * enbw_bins`
/// holds against [`to_psd`] (see the module docs).
///
/// Panics if `sum(w)` is not positive.
pub fn to_autopower(raw: &mut [f64], window: &[f64]) {
    let s1: f64 = window.iter().sum();
    assert!(s1 > 0.0, "to_autopower: window sum must be positive");
    let norm = 1.0 / (s1 * s1);
    for p in raw.iter_mut() {
        *p *= norm;
    }
}
