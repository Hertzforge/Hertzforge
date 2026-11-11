//! Closed-form cases of the brief's section 8 that need a plant or a large
//! sample rather than one module: V3 (a swept tone through a resonance, open
//! loop) and V5 (the peak statistics of a sine plus Gaussian noise).

use crate::predict::sine_gauss_exceedance;
use crate::testkit::Gauss;
use std::f64::consts::TAU;

/// V5: a tone of 3 sigma plus Gaussian noise of rms sigma, 1e6 samples: the
/// share beyond 4, 5 and 6 sigma is within the binomial 99 % bounds of the
/// closed-form integral.
#[test]
fn v5_the_peaks_of_a_sine_plus_noise_follow_the_integral() {
    let n = 1_000_000usize;
    let mut g = Gauss::new(17);
    let (a, sigma) = (3.0, 1.0);
    let f = 0.123_456_7; // cycles per sample, incommensurate
    let mut over = [0usize; 3];
    for i in 0..n {
        let x = a * (TAU * f * i as f64).sin() + sigma * g.next();
        for (k, c) in [4.0, 5.0, 6.0].iter().enumerate() {
            if x.abs() > *c {
                over[k] += 1;
            }
        }
    }
    for (k, c) in [4.0f64, 5.0, 6.0].iter().enumerate() {
        let p = sine_gauss_exceedance(*c, a, sigma);
        let mean = p * n as f64;
        let band = 2.576 * (n as f64 * p * (1.0 - p)).sqrt();
        assert!(
            (over[k] as f64 - mean).abs() <= band + 1.0,
            "{c} sigma: {} against {mean:.1} +/- {band:.1}",
            over[k]
        );
    }
    // No tone: the Gaussian tail itself.
    let p3 = sine_gauss_exceedance(3.0, 0.0, 1.0);
    assert!((p3 - 0.002_699_8).abs() < 1e-6, "{p3}");
}

/// An SDOF response to a constant-amplitude linear sweep from `f0` to `f1` at
/// `r` Hz/s (negative: down): the largest envelope and the drive frequency
/// where it falls. The envelope is `sqrt(x^2 + (v / w_n)^2)`, read the same
/// way for every rate so its own ripple cancels in a ratio.
fn swept_sdof_peak(fn_hz: f64, q: f64, f0: f64, f1: f64, r: f64) -> (f64, f64) {
    let fs = 20_000.0;
    let dt = 1.0 / fs;
    let wn = TAU * fn_hz;
    let z = 1.0 / (2.0 * q);
    let (mut x, mut v) = (0.0f64, 0.0f64);
    let t_end = (f1 - f0) / r;
    let mut t = 0.0;
    let phase = |t: f64| TAU * (f0 * t + 0.5 * r * t * t);
    let acc = |x: f64, v: f64, t: f64| wn * wn * phase(t).sin() - 2.0 * z * wn * v - wn * wn * x;
    let (mut peak, mut at) = (0.0, f0);
    while t < t_end {
        // RK4.
        let (k1x, k1v) = (v, acc(x, v, t));
        let (k2x, k2v) = (
            v + 0.5 * dt * k1v,
            acc(x + 0.5 * dt * k1x, v + 0.5 * dt * k1v, t + 0.5 * dt),
        );
        let (k3x, k3v) = (
            v + 0.5 * dt * k2v,
            acc(x + 0.5 * dt * k2x, v + 0.5 * dt * k2v, t + 0.5 * dt),
        );
        let (k4x, k4v) = (v + dt * k3v, acc(x + dt * k3x, v + dt * k3v, t + dt));
        x += dt / 6.0 * (k1x + 2.0 * k2x + 2.0 * k3x + k4x);
        v += dt / 6.0 * (k1v + 2.0 * k2v + 2.0 * k3v + k4v);
        t += dt;
        let env = (x * x + (v / wn) * (v / wn)).sqrt();
        if env > peak {
            peak = env;
            at = f0 + r * t;
        }
    }
    (peak, at)
}

/// V3, open loop: Q = 20, f_n = 100 Hz, swept up and down at the brief's
/// k = 0.05 (r = 4 pi f_n^2 k / Q^2 = 15.7 Hz/s).
///
/// **The measurement disagrees with the brief, and wins.** The brief reads
/// Cowan et al. as a peak of 0.9962 of the steady state, 0.5 Hz late. The
/// direct solution of the oscillator (RK4 at 20 kHz, its steady state checked
/// against Q at a 0.2 Hz/s crawl) gives 0.967 and 1.6 Hz late, and its rate
/// dependence has the form the paper's series has (the loss grows as k^2 and
/// the delay as k at small k, both vanish as the rate does), so the brief's
/// normalization of k is what is off, by about a factor of 2.5. The loss
/// agrees with the classical empirical sweep-loss curve (about 3 % at a
/// sweep parameter r Q^2 / f_n^2 of 0.63). These numbers are pinned here; the
/// closed-loop V3 is judged against the tone tolerance, not the series.
#[test]
fn v3_a_swept_resonance_peaks_low_and_late() {
    let (fn_hz, q) = (100.0, 20.0);
    let rate = |k: f64| 4.0 * std::f64::consts::PI * fn_hz * fn_hz * k / (q * q);
    assert!((rate(0.05) - 15.708).abs() < 1e-3);
    let (ss, f_ss) = swept_sdof_peak(fn_hz, q, 95.0, 105.0, 0.2);
    assert!((ss / (q / (1.0 - 1.0 / (4.0 * q * q)).sqrt()) - 1.0).abs() < 1e-3);
    let (up, f_up) = swept_sdof_peak(fn_hz, q, 60.0, 140.0, rate(0.05));
    let (dn, f_dn) = swept_sdof_peak(fn_hz, q, 140.0, 60.0, -rate(0.05));
    assert!((up / ss - 0.967).abs() < 0.005, "up {:.4}", up / ss);
    assert!((up / dn - 1.0).abs() < 0.002, "up and down alike");
    assert!(
        (f_up - f_ss - 1.6).abs() < 0.25,
        "late going up: {:.3}",
        f_up - f_ss
    );
    assert!(
        (f_ss - f_dn - 1.6).abs() < 0.25,
        "late going down: {:.3}",
        f_ss - f_dn
    );
    // The series' form: loss ~ k^2 and delay ~ k as the rate falls.
    let at = |k: f64| {
        let (a, f) = swept_sdof_peak(fn_hz, q, 60.0, 140.0, rate(k));
        (1.0 - a / ss, f - f_ss)
    };
    let (l1, d1) = at(0.0125);
    let (l2, d2) = at(0.025);
    assert!(l2 / l1 > 3.0 && l2 / l1 < 4.5, "loss {l1} -> {l2}");
    assert!(d2 / d1 > 1.6 && d2 / d1 < 2.1, "delay {d1} -> {d2}");
    assert!(l1 < 0.004);
}
