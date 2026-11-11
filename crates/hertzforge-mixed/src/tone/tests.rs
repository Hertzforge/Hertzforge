use super::*;
use crate::testkit::Gauss;
use std::f64::consts::{PI, TAU};

const FS: f64 = 1024.0;

#[test]
fn the_tracker_noise_bandwidth_is_the_width_asked_for() {
    for bn in [0.25, 0.5, 2.0] {
        let t = ToneTracker::new(FS, bn);
        let got = t.exact_noise_bandwidth_hz();
        assert!((got / bn - 1.0).abs() < 0.01, "asked {bn}, got {got}");
    }
}

/// V1: a fixed tone of 1 m/s^2 peak at 100 Hz in white noise of 1e-3
/// (m/s^2)^2/Hz, tracker 0.5 Hz. One reading scatters by sqrt(G B_n) =
/// 0.0224; the mean over T seconds is within 3 sqrt(G / T) of A. Readings for
/// the scatter are taken 2 / B_n = 4 s apart (independent); 1000 of them.
#[test]
fn v1_a_tone_in_white_noise_reads_with_the_closed_form_scatter_and_mean() {
    let (a, f, g, bn) = (1.0, 100.0, 1e-3, 0.5);
    let mut t = ToneTracker::new(FS, bn);
    let mut noise = Gauss::new(11);
    let settle = (t.settle_s() * 4.0 * FS) as usize;
    let spacing = (2.0 / bn * FS) as usize;
    let n_read = 1000;
    let total = settle + spacing * n_read;
    let (mut spaced, mut sum, mut count) = (Vec::with_capacity(n_read), 0.0, 0usize);
    let every = (0.1 * FS) as usize;
    for n in 0..total {
        let phase = TAU * f * n as f64 / FS;
        t.update(a * phase.sin() + noise.white(g, FS), phase);
        if n >= settle {
            let k = n - settle;
            if (k + 1).is_multiple_of(spacing) {
                spaced.push(t.amplitude());
            }
            if k.is_multiple_of(every) {
                sum += t.amplitude();
                count += 1;
            }
        }
    }
    let mean_spaced = spaced.iter().sum::<f64>() / spaced.len() as f64;
    let std = (spaced
        .iter()
        .map(|x| (x - mean_spaced).powi(2))
        .sum::<f64>()
        / (spaced.len() - 1) as f64)
        .sqrt();
    let want = (g * bn).sqrt();
    assert!(
        (std / want - 1.0).abs() < 0.2,
        "per-reading std {std:.5} against {want:.5}"
    );
    let t_run = (total - settle) as f64 / FS;
    let mean = sum / count as f64;
    let band = 3.0 * (g / t_run).sqrt();
    assert!(
        (mean - a).abs() < band,
        "mean {mean:.5} against {a} +/- {band:.5}"
    );
}

/// The settle rule: at 1.3 / B_n a step reads within 3.5 % (the four-pole
/// cascade's step at 8.32 time constants is 0.966); at 0.9 / B_n it does not.
#[test]
fn a_step_settles_within_the_settle_time_and_not_before() {
    let bn = 0.5;
    let mut t = ToneTracker::new(FS, bn);
    let f = 80.0;
    let n_early = (0.9 / bn * FS) as usize;
    let n_settle = (t.settle_s() * FS).ceil() as usize;
    let mut early = 0.0;
    for n in 0..n_settle {
        let phase = TAU * f * n as f64 / FS;
        t.update(phase.sin(), phase);
        if n + 1 == n_early {
            early = t.amplitude();
            assert!(!t.settled());
        }
    }
    assert!(t.settled());
    assert!(t.amplitude() > 0.965, "{}", t.amplitude());
    assert!(early < 0.9, "{early}");
}

/// Two tones of equal amplitude 2 B_n apart: each tracker reads its own within
/// the cascade's rejection, about 4.2 % at that spacing (under 5 %).
#[test]
fn a_second_tone_two_widths_away_moves_a_reading_by_under_five_percent() {
    let bn = 0.5;
    let mut t = ToneTracker::new(FS, bn);
    let (f1, f2) = (100.0, 100.0 + 2.0 * bn);
    let n_settle = (8.0 / bn * FS) as usize;
    let mut worst: f64 = 0.0;
    for n in 0..n_settle * 2 {
        let tt = n as f64 / FS;
        let p1 = TAU * f1 * tt;
        t.update(p1.sin() + (TAU * f2 * tt).sin(), p1);
        if n > n_settle {
            worst = worst.max((t.amplitude() - 1.0).abs());
        }
    }
    assert!(worst < 0.05, "{worst}");
    assert!(worst > 0.02, "the beat is there to be bounded: {worst}");
}

/// Subtraction (D5): a 10-peak tone in noise of 1e-3, tracked, subtracted;
/// a fresh tracker on the residual reads what is left of the tone at under
/// 1 % of it, against the noise's own reading of sqrt(pi G B_n / 2).
#[test]
fn the_passed_waveform_takes_the_tone_out_of_the_residual() {
    let (a, f, g, bn) = (10.0, 150.0, 1e-3, 0.5);
    let mut t = ToneTracker::new(FS, bn);
    let mut check = ToneTracker::new(FS, bn);
    let mut noise = Gauss::new(3);
    let n_total = (60.0 * FS) as usize;
    let mut left = Vec::new();
    for n in 0..n_total {
        let phase = TAU * f * n as f64 / FS;
        let x = a * (phase + 0.7).sin() + noise.white(g, FS);
        t.update(x, phase);
        let r = x - t.passed(phase);
        if n > (10.0 * FS) as usize {
            check.update(r, phase);
            if n > (20.0 * FS) as usize && n % 1024 == 0 {
                left.push(check.amplitude());
            }
        }
    }
    let mean_left = left.iter().sum::<f64>() / left.len() as f64;
    let rayleigh = (PI * g * bn / 2.0).sqrt();
    assert!(mean_left < 0.01 * a, "{mean_left}");
    assert!(mean_left < 3.0 * rayleigh, "{mean_left} against {rayleigh}");
}

/// The bank reads the response the drive made `lag` frames earlier: through a
/// pure delay of `lag` frames, a tone reads its amplitude at zero phase; read
/// one frame off, its phase is off by 2 pi f / fs.
#[test]
fn the_trackers_read_against_the_phase_played_lag_frames_before() {
    let lag = 37;
    for (bank_lag, want_phase) in [(lag, 0.0), (lag - 1, -TAU * 60.0 / FS)] {
        let mut bank = ToneBank::new(FS, &[60.0], 1, 0.5, bank_lag);
        bank.voice_mut(0).set_amplitude(2.0);
        bank.voice_mut(0).ramp_to(1.0, 0.0);
        let mut delay = std::collections::VecDeque::from(vec![0.0; lag]);
        let mut res = [0.0];
        for _ in 0..(10.0 * FS) as usize {
            let d = bank.drive_frame(1.0);
            delay.push_back(d);
            let x = delay.pop_front().unwrap();
            bank.track_frame(&[x], &mut res);
        }
        let t = bank.tracker(0, 0);
        assert!((t.amplitude() - 2.0).abs() < 1e-3, "{}", t.amplitude());
        let err = (t.phase() - want_phase + PI).rem_euclid(TAU) - PI;
        assert!(err.abs() < 1e-3, "lag {bank_lag}: phase {}", t.phase());
        assert!(res[0].abs() < 1e-2, "residual {}", res[0]);
    }
}

/// Phase continuity at the output: the drive is the same sample for sample
/// however the frames are split into blocks (frame, chunk and loop
/// boundaries are block boundaries), and through a frequency jump, a sweep,
/// an amplitude slew, a part ramp in and out, and a level ramp, no sample
/// moves more than the largest slope a sine of that amplitude and frequency
/// has, plus the ramps' own steps.
#[test]
fn every_tone_is_phase_continuous_across_every_boundary() {
    let run = |blocks: &[usize]| {
        let mut bank = ToneBank::new(FS, &[50.0, 120.0], 1, 0.5, 0);
        for i in 0..2 {
            bank.voice_mut(i).set_amplitude(1.0);
            bank.voice_mut(i).ramp_to(1.0, 0.0);
        }
        let mut out = Vec::new();
        let mut level = Ramp::at(0.5);
        let mut n = 0usize;
        let mut res = [0.0];
        for &b in blocks {
            for _ in 0..b {
                // A frequency jump at 1 s, a log-ish sweep from 2 s, a slew at
                // 3 s, the second part out at 4 s and back in at 5 s, a level
                // ramp from 6 s.
                let t = n as f64 / FS;
                if n == FS as usize {
                    bank.voice_mut(0).set_frequency(75.0);
                }
                if t >= 2.0 {
                    let f = 120.0 * 2f64.powf((t - 2.0) / 4.0);
                    bank.voice_mut(1).set_frequency(f.min(240.0));
                }
                if n == 3 * FS as usize {
                    bank.voice_mut(0).slew_to(2.0, 0.01 * FS);
                }
                if n == 4 * FS as usize {
                    bank.voice_mut(1).ramp_to(0.0, 0.5 * FS);
                }
                if n == 5 * FS as usize {
                    bank.voice_mut(1).ramp_to(1.0, 0.5 * FS);
                }
                if n == 6 * FS as usize {
                    level.go(1.0, FS);
                }
                let x = bank.drive_frame(level.next());
                bank.track_frame(&[x], &mut res);
                out.push(x);
                n += 1;
            }
        }
        out
    };
    let total = 8 * FS as usize;
    let whole = run(&[total]);
    let mut odd = Vec::new();
    let mut left = total;
    for b in [1usize, 7, 64, 333, 1024, 4096].iter().cycle() {
        if left == 0 {
            break;
        }
        let b = (*b).min(left);
        odd.push(b);
        left -= b;
    }
    assert_eq!(whole, run(&odd), "block boundaries change the drive");
    // The largest slope: two tones of at most 2 at up to 240 Hz.
    let bound = TAU * (2.0 * 75.0 + 1.0 * 240.0) / FS * 1.05;
    let worst = whole
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f64::max);
    assert!(worst <= bound, "a step of {worst} against {bound}");
}

/// A part ramped out reaches exactly zero and stays there; ramped back in it
/// returns at its amplitude, never above.
#[test]
fn a_part_ramp_ends_at_zero_and_never_overshoots() {
    let mut v = ToneVoice::new(FS, 100.0);
    v.set_amplitude(1.0);
    v.ramp_to(1.0, 10.0);
    let mut peak: f64 = 0.0;
    for _ in 0..200 {
        peak = peak.max(v.next().0.abs());
    }
    assert!(peak <= 1.0 + 1e-12);
    v.ramp_to(0.0, 100.0);
    for _ in 0..100 {
        v.next();
    }
    for _ in 0..1000 {
        assert_eq!(v.next().0, 0.0);
    }
    assert_eq!(v.scale(), 0.0);
}

/// V7: the tone loop's stability bound, tested, not trusted. With a reading
/// `d` loops late on a pure-gain plant, a log-proportional loop after a 6 dB
/// step converges under the bound, holds an undamped oscillation at it, and
/// grows over it, to +/- 0.02 in c.
#[test]
fn v7_the_tone_loop_is_stable_exactly_under_its_bound() {
    let simulate = |c: f64, d: usize| -> (f64, f64) {
        // e[n] = ln(A_ref / A_hat[n]); the reading lags the drive by d loops.
        let mut ln_a = vec![0.0f64; d + 1];
        let ln_ref = 6.0 / 20.0 * std::f64::consts::LN_10;
        let mut early: f64 = 0.0;
        let mut late: f64 = 0.0;
        let n = 3000;
        for k in 0..n {
            let read = ln_a[ln_a.len() - 1 - d];
            let e = ln_ref - read;
            if (200..400).contains(&k) {
                early = early.max(e.abs());
            }
            if k >= n - 200 {
                late = late.max(e.abs());
            }
            let next = ln_a[ln_a.len() - 1] + c * e;
            ln_a.push(next);
        }
        (early, late)
    };
    for d in 0..3 {
        let c0 = tone_loop_gain_bound(d);
        let (e, l) = simulate(c0 - 0.02, d);
        assert!(
            l < 0.5 * e,
            "d {d}: c {} should converge ({e} -> {l})",
            c0 - 0.02
        );
        let (e, l) = simulate(c0 + 0.02, d);
        assert!(
            l > 2.0 * e,
            "d {d}: c {} should grow ({e} -> {l})",
            c0 + 0.02
        );
        let (e, l) = simulate(c0, d);
        assert!(l > 0.5 * e && l < 2.0 * e, "d {d}: at the bound {e} -> {l}");
    }
    assert!((tone_loop_gain_bound(0) - 2.0).abs() < 1e-12);
    assert!((tone_loop_gain_bound(1) - 1.0).abs() < 1e-12);
    assert!((tone_loop_gain_bound(2) - 0.618_034).abs() < 1e-6);
}

#[test]
fn a_tone_under_the_floor_in_its_band_is_not_readable() {
    // 1 g peak in 0.01 g^2/Hz at 0.5 Hz: 0.5 / 0.005 = 100, readable.
    assert!(tone_readable(1.0, 0.01, 0.5));
    // 0.2 g: 0.02 / 0.005 = 4, not.
    assert!(!tone_readable(0.2, 0.01, 0.5));
}
