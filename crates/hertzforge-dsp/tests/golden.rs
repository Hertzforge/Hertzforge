//! Golden-vector tests for the hertzforge-dsp analysis chain.
//!
//! Every reference here is ANALYTIC (closed-form), never a value copied out of
//! the same Rust code under test, so the test and the implementation cannot
//! silently co-confirm. Where a second, independent check is cheap (Parseval,
//! variance-of-a-sine) it is included so a wrong normalization constant is
//! caught by more than one route.
//!
//! The independent numeric cross-check against numpy/scipy lives in
//! `tools/dsp-xref/xref.py`; this file is the in-tree Rust half.

use hertzforge_dsp::{fft, integrate, spectrum, srs};
use std::f64::consts::TAU;

// ---------------------------------------------------------------------------
// FFT
// ---------------------------------------------------------------------------

/// A pure tone at an integer bin puts all its energy in that bin (and its
/// conjugate mirror), with the analytic magnitude N/2 for a unit cosine.
#[test]
fn fft_tone_energy_in_expected_bin() {
    let n = 256usize;
    let m = 17usize; // integer bin -> no leakage
    let x: Vec<f64> = (0..n)
        .map(|k| (TAU * m as f64 * k as f64 / n as f64).cos())
        .collect();
    let spec = fft::fft(&fft::zero_pad_pow2(&x));
    for (k, &c) in spec.iter().enumerate() {
        let mag = fft::mag(c);
        if k == m || k == n - m {
            // unit cosine -> N/2 in each of the two bins
            assert!(
                (mag - n as f64 / 2.0).abs() < 1e-6,
                "expected N/2 at bin {k}, got {mag}"
            );
        } else {
            assert!(mag < 1e-6, "unexpected energy {mag} at bin {k}");
        }
    }
}

/// Parseval / Rayleigh: sum |x[n]|^2 == (1/N) sum |X[k]|^2.
#[test]
fn fft_parseval_holds() {
    let n = 512usize;
    // a nontrivial mixed signal
    let x: Vec<f64> = (0..n)
        .map(|k| {
            let t = k as f64;
            0.7 * (TAU * 3.0 * t / n as f64).sin() + 0.4 * (TAU * 40.0 * t / n as f64).cos() + 0.1
        })
        .collect();
    let time_energy: f64 = x.iter().map(|v| v * v).sum();
    let spec = fft::fft(&fft::zero_pad_pow2(&x));
    let freq_energy: f64 = spec.iter().map(|&c| fft::mag(c).powi(2)).sum::<f64>() / n as f64;
    let rel = (time_energy - freq_energy).abs() / time_energy;
    assert!(rel < 1e-12, "Parseval violated: rel err {rel}");
}

/// ifft(fft(x)) reconstructs x to float precision, complex input.
#[test]
fn fft_roundtrip_is_identity() {
    let n = 1024usize;
    let x: Vec<fft::Cplx> = (0..n)
        .map(|k| {
            let t = k as f64;
            (
                (0.11 * t).sin() - 0.3 * (0.02 * t).cos(),
                0.2 * (0.05 * t).cos(),
            )
        })
        .collect();
    let back = fft::ifft(&fft::fft(&x));
    let mut worst = 0.0f64;
    for (a, b) in x.iter().zip(back.iter()) {
        worst = worst.max((a.0 - b.0).abs()).max((a.1 - b.1).abs());
    }
    assert!(worst < 1e-11, "roundtrip worst abs err {worst}");
}

// ---------------------------------------------------------------------------
// Welch PSD
// ---------------------------------------------------------------------------

/// A unit-amplitude sine has variance 1/2. Integrating the single-sided PSD
/// over frequency must recover that variance (Parseval for the density), and
/// the peak of the PSD must sit at the sine's frequency.
#[test]
fn psd_of_unit_sine_integrates_to_half_and_peaks_at_f() {
    let fs = 1024.0;
    let f0 = 64.0; // integer number of cycles per segment -> minimal leakage
    let n = 1 << 16; // 65536 samples
    let x: Vec<f64> = (0..n).map(|k| (TAU * f0 * k as f64 / fs).sin()).collect();

    let nfft = 4096usize;
    let (freqs, psd) = spectrum::psd_welch(&x, fs, nfft, 0.5);
    assert_eq!(freqs.len(), nfft / 2 + 1);

    // integral of the one-sided density == variance of the sine == 0.5
    let df = fs / nfft as f64;
    let area: f64 = psd.iter().sum::<f64>() * df;
    assert!(
        (area - 0.5).abs() < 0.02,
        "sine variance from PSD = {area}, expected 0.5"
    );

    // peak bin sits at f0
    let (peak_k, _) =
        psd.iter().enumerate().fold(
            (0usize, 0.0f64),
            |(bk, bv), (k, &v)| {
                if v > bv {
                    (k, v)
                } else {
                    (bk, bv)
                }
            },
        );
    assert!(
        (freqs[peak_k] - f0).abs() < df,
        "PSD peak at {} Hz, expected {f0} Hz",
        freqs[peak_k]
    );
}

/// White-ish flatness sanity: a DC offset must NOT show up (per-segment mean is
/// removed) and the density stays finite. Also checks the DC bin is not doubled
/// by feeding a pure sine plus a large offset and confirming variance recovery
/// is unaffected by the offset.
#[test]
fn psd_ignores_dc_offset() {
    let fs = 1000.0;
    let f0 = 50.0;
    let n = 1 << 15;
    let offset = 123.4;
    let x: Vec<f64> = (0..n)
        .map(|k| offset + (TAU * f0 * k as f64 / fs).sin())
        .collect();
    let nfft = 2048usize;
    let (_freqs, psd) = spectrum::psd_welch(&x, fs, nfft, 0.5);
    let df = fs / nfft as f64;
    let area: f64 = psd.iter().sum::<f64>() * df;
    // still the sine variance, the DC is gone
    assert!(
        (area - 0.5).abs() < 0.03,
        "variance with DC offset removed = {area}, expected ~0.5"
    );
}

// ---------------------------------------------------------------------------
// SRS (Smallwood ramp-invariant, maxi-max absolute acceleration)
// ---------------------------------------------------------------------------

/// Build a half-sine base-acceleration pulse: peak `amp`, duration `tau`,
/// followed by quiet time so the residual response develops fully.
fn half_sine_pulse(amp: f64, tau: f64, fs: f64, quiet: f64) -> Vec<f64> {
    let n_pulse = (tau * fs).round() as usize;
    let n_quiet = (quiet * fs).round() as usize;
    let mut v = Vec::with_capacity(n_pulse + n_quiet);
    for k in 0..n_pulse {
        let t = k as f64 / fs;
        v.push(amp * (std::f64::consts::PI * t / tau).sin());
    }
    v.extend(std::iter::repeat_n(0.0, n_quiet));
    v
}

/// High-frequency asymptote: for f_n * tau >> 1 the SDOF rigidly follows the
/// base, so the maxi-max SRS approaches the peak input acceleration `amp`.
#[test]
fn srs_half_sine_high_freq_asymptote_is_input_peak() {
    let fs = 20_000.0;
    let amp = 10.0; // g
    let tau = 0.005; // 5 ms half-sine -> primary spectrum knee near 1/tau = 200 Hz
    let pulse = half_sine_pulse(amp, tau, fs, 0.05);

    // f_n well above 1/tau: 4000 Hz -> f_n*tau = 20
    let freqs = [4000.0];
    let q = 10.0;
    let out = srs::srs_analyze(&pulse, fs, &freqs, q);
    // rigid-follow region: within a few percent of the input peak
    assert!(
        (out[0] - amp).abs() / amp < 0.05,
        "high-freq SRS {} should approach input peak {amp}",
        out[0]
    );
}

/// Low-frequency asymptote (velocity regime): for f_n * tau << 1 the undamped
/// maxi-max SRS asymptotes to omega_n * dV, where dV is the base velocity change
/// of the pulse. For a half-sine of peak `amp` and duration `tau`,
/// dV = amp * 2*tau/pi, so SRS -> 2*pi*f_n * (2*amp*tau/pi) = 4*amp*tau*f_n.
/// This is a genuine textbook asymptote, checked as a proportionality.
#[test]
fn srs_half_sine_low_freq_velocity_asymptote() {
    let fs = 50_000.0;
    let amp = 10.0; // g
    let tau = 0.005; // s
                     // long quiet tail so the low-freq residual (slow) fully develops
    let pulse = half_sine_pulse(amp, tau, fs, 2.0);

    let q = 50.0; // light damping -> close to undamped asymptote
    let f_lo = 2.0; // f_n*tau = 0.01 << 1
    let out = srs::srs_analyze(&pulse, fs, &[f_lo], q);

    let dv = amp * 2.0 * tau / std::f64::consts::PI;
    let expected = TAU * f_lo * dv; // omega_n * dV
    let rel = (out[0] - expected).abs() / expected;
    assert!(
        rel < 0.10,
        "low-freq SRS {} vs velocity asymptote {expected} (rel {rel})",
        out[0]
    );
}

/// A sustained sine at the SDOF natural frequency drives the response to ~Q
/// times the input amplitude (steady-state resonance gain of a SDOF).
#[test]
fn srs_single_dof_at_resonance_shows_q_amplification() {
    let fs = 10_000.0;
    let fn_hz = 100.0;
    let q = 20.0;
    let amp = 1.0; // g
                   // many cycles so the resonant build-up reaches steady state (~Q cycles)
    let n = (fs * (3.0 * q / fn_hz)) as usize;
    let drive: Vec<f64> = (0..n)
        .map(|k| amp * (TAU * fn_hz * k as f64 / fs).sin())
        .collect();

    let out = srs::srs_analyze(&drive, fs, &[fn_hz], q);
    // steady-state gain at resonance is Q; allow generous tolerance for the
    // finite build-up and the discrete filter.
    let gain = out[0] / amp;
    assert!(
        (gain - q).abs() / q < 0.10,
        "resonance gain {gain} should be ~Q={q}"
    );
}

/// SRS is monotone-ish from the low-frequency (small) toward the input-peak
/// plateau: the lowest frequency reads well below the input peak, the highest
/// reads near it. Guards against a filter that returns a constant.
#[test]
fn srs_spectrum_rises_from_low_to_plateau() {
    let fs = 20_000.0;
    let amp = 8.0;
    let tau = 0.004;
    let pulse = half_sine_pulse(amp, tau, fs, 0.3);
    let freqs = [5.0, 50.0, 250.0, 1000.0, 5000.0];
    let out = srs::srs_analyze(&pulse, fs, &freqs, 10.0);
    // lowest freq far below peak
    assert!(out[0] < 0.2 * amp, "low-f SRS {} not small", out[0]);
    // highest freq near the input peak
    assert!(
        (out[4] - amp).abs() / amp < 0.1,
        "high-f SRS {} not near peak {amp}",
        out[4]
    );
    // near the knee (1/tau = 250 Hz) the primary SRS overshoots the input peak
    // (classic half-sine amplification ~1.7x); assert it exceeds the peak.
    assert!(out[2] > amp, "knee SRS {} should exceed input peak", out[2]);
}

// ---------------------------------------------------------------------------
// accel_to_velocity (trapezoid + banded high-pass)
// ---------------------------------------------------------------------------

/// Integrating A*sin(wt) [g] yields -(G*A/w)*cos(wt) [in/s] once the high-pass
/// transient settles. Check both the amplitude and the -cos phase.
#[test]
fn accel_to_velocity_sine_matches_analytic() {
    let fs = 10_000.0;
    let f = 100.0; // well above the HP corner
    let a = 2.0; // g amplitude
    let w = TAU * f;
    let n = 40_000; // 4 s -> HP settles in the first fraction of a second
    let accel: Vec<f64> = (0..n).map(|k| a * (w * k as f64 / fs).sin()).collect();

    let hp = 2.0; // Hz corner, 50x below the signal -> passband gain ~1
    let vel = integrate::accel_to_velocity(&accel, fs, hp);

    // expected: v(t) = -(G*A/w) cos(wt); amplitude:
    let expected_amp = integrate::G_TO_IN_S2 * a / w;

    // measure amplitude + phase in the settled tail (last second)
    let tail_start = n - (fs as usize);
    let mut vmax = 0.0f64;
    for &v in &vel[tail_start..] {
        vmax = vmax.max(v.abs());
    }
    let rel = (vmax - expected_amp).abs() / expected_amp;
    assert!(
        rel < 0.02,
        "velocity amplitude {vmax} vs analytic {expected_amp} (rel {rel})"
    );

    // phase: at a sample where sin(wt) ~ 0 rising (wt = 2*pi*m), expect
    // v ~ -amp (cos = 1). Find such a sample in the tail.
    // pick t where wt is an integer multiple of 2*pi.
    let cycles_per_sample = w / fs;
    // sample index nearest an integer number of cycles inside the tail
    let mut best_k = tail_start;
    let mut best_frac = 1.0;
    for k in tail_start..n {
        let phase = (k as f64 * cycles_per_sample) / TAU;
        let frac = (phase - phase.round()).abs();
        if frac < best_frac {
            best_frac = frac;
            best_k = k;
        }
    }
    // at wt = 2*pi*m: -cos = -1, so v should be near -expected_amp
    let v_here = vel[best_k];
    assert!(
        v_here < 0.0 && (v_here + expected_amp).abs() / expected_amp < 0.05,
        "phase check: v at cycle boundary = {v_here}, expected ~{}",
        -expected_amp
    );
}

/// The high-pass removes a DC drift: a constant-acceleration input would ramp
/// velocity linearly forever without the HP; with the HP the settled velocity
/// is bounded (drift killed).
#[test]
fn accel_to_velocity_kills_dc_drift() {
    let fs = 1000.0;
    let n = 20_000;
    // constant 1 g bias -> raw integral ramps to G*1*20s = ~7720 in/s
    let accel = vec![1.0f64; n];
    let hp = 1.0;
    let vel = integrate::accel_to_velocity(&accel, fs, hp);
    // settled tail should be near zero, not a 7000+ ramp
    let tail = &vel[n - 1000..];
    let tail_max = tail.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
    assert!(
        tail_max < 5.0,
        "HP failed to kill DC drift: tail max {tail_max} in/s"
    );
}
