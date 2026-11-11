use super::*;
use crate::testkit::Gauss;
use crate::tone::ToneTracker;
use hertzforge_dsp::cola::ColaSynth;
use std::f64::consts::{PI, TAU};

/// Hann leakage at offset `x` lines from a tone (the brief's 3.2).
fn hann_w(x: f64) -> f64 {
    let s = (PI * x).sin() / (PI * x);
    (s / (1.0 - x * x)).abs()
}

/// V6: a 10-peak tone halfway between 1 Hz lines leaks into the Hann
/// periodogram as `(A^2/2) W^2(x) / (1.5 df)` at x = 2.5, 3.5, 4.5 lines
/// (within 1 dB); the exclusion inequality's answer for eps = 0.1 on a 0.01
/// floor is x = 4 (leakage under 0.1 G there, over it at 3); subtracting the
/// tone to 1 % takes its leakage down 40 dB.
#[test]
fn v6_leakage_follows_the_hann_formula_and_subtraction_cuts_it_forty_db() {
    let (fs, n) = (1024.0, 1024usize);
    let (a, f) = (10.0, 100.5);
    let win = hann(n);
    let x: Vec<f64> = (0..n)
        .map(|i| a * (TAU * f * i as f64 / fs).sin())
        .collect();
    let mut scratch = Vec::new();
    let mut p = vec![0.0; n / 2 + 1];
    periodogram(&x, &win, fs, &mut scratch, &mut p);
    for off in [2.5f64, 3.5, 4.5] {
        let line = (f + off) as usize;
        let want = a * a / 2.0 * hann_w(off).powi(2) / 1.5;
        let err_db = 10.0 * (p[line] / want).log10();
        assert!(
            err_db.abs() < 1.0,
            "x {off}: {} against {want} ({err_db:+.2} dB)",
            p[line]
        );
    }
    // The inequality x (x^2 - 1) >= sqrt(A^2 / (3 eps G df)) / pi.
    let (g, eps) = (0.01, 0.1);
    let rhs = (a * a / (3.0 * eps * g)).sqrt() / PI;
    let x_need = (2..10)
        .map(|k| k as f64)
        .find(|x| x * (x * x - 1.0) >= rhs)
        .unwrap();
    assert_eq!(x_need, 4.0);
    let env = |x: f64| a * a / 2.0 * (1.0 / (PI * x * (x * x - 1.0))).powi(2) / 1.5;
    assert!(env(4.0) <= eps * g && env(3.0) > eps * g);
    // A 1 % subtraction error leaves a hundredth of the amplitude.
    let r: Vec<f64> = x.iter().map(|v| 0.01 * v).collect();
    let mut pr = vec![0.0; n / 2 + 1];
    periodogram(&r, &win, fs, &mut scratch, &mut pr);
    let line = (f + 2.5) as usize;
    assert!((10.0 * (pr[line] / p[line]).log10() + 40.0).abs() < 1e-6);
}

/// V2: a flat 0.01 g^2/Hz floor from 20 to 2000 Hz (19.8), tones of 2 and 1
/// peak (2 + 0.5) and a 10 Hz band of 0.05 added (0.5): the composite mean
/// square is 22.8 within 2 %; with both tones tracked and subtracted the
/// residual's is the floor's and the band's, 20.3, within 2 %; and the
/// periodogram integral of the residual equals its time mean square.
#[test]
fn v2_mean_squares_add_and_the_residual_is_the_random_part() {
    let (fs, nfft) = (4096.0, 4096usize);
    let mut psd = vec![0.0; nfft / 2 + 1];
    for (k, p) in psd.iter_mut().enumerate() {
        let f = k as f64;
        if (20.0..2000.0).contains(&f) {
            *p = 0.01;
        }
        if (995.0..1005.0).contains(&f) {
            *p += 0.05;
        }
    }
    let mut synth = ColaSynth::with_seed(nfft, 0.5, fs, 5);
    let secs = 20.0;
    let mut random = Vec::new();
    while random.len() < (secs * fs) as usize {
        random.extend(synth.next_frame(&psd));
    }
    let tones = [(2.0, 100.0), (1.0, 300.0)];
    let mut tr: Vec<ToneTracker> = tones.iter().map(|_| ToneTracker::new(fs, 0.5)).collect();
    let skip = (5.0 * fs) as usize;
    let (mut ms_all, mut ms_res, mut count) = (0.0, 0.0, 0usize);
    let mut residual = Vec::new();
    for (i, r) in random.iter().enumerate() {
        let t = i as f64 / fs;
        let mut x = *r;
        for (a, f) in tones {
            x += a * (TAU * f * t).sin();
        }
        let mut res = x;
        for ((_, f), trk) in tones.iter().zip(tr.iter_mut()) {
            let ph = TAU * f * t;
            trk.update(x, ph);
            res -= trk.passed(ph);
        }
        if i >= skip {
            ms_all += x * x;
            ms_res += res * res;
            residual.push(res);
            count += 1;
        }
    }
    ms_all /= count as f64;
    ms_res /= count as f64;
    assert!((ms_all / 22.8 - 1.0).abs() < 0.02, "composite {ms_all}");
    assert!((ms_res / 20.3 - 1.0).abs() < 0.02, "residual {ms_res}");
    let (_, w) = hertzforge_dsp::spectrum::psd_welch(&residual, fs, nfft, 0.5);
    let integral: f64 = w.iter().sum::<f64>() * fs / nfft as f64;
    assert!(
        (integral / ms_res - 1.0).abs() < 0.02,
        "{integral} against {ms_res}"
    );
}

#[test]
fn the_estimate_averages_clean_frames_only_with_its_own_dof_per_line() {
    let mut e = FloorEstimator::new(8, 100);
    let mut ex = vec![false; 8];
    for f in 0..10 {
        ex[3] = f % 2 == 0; // line 3 is out on even frames
        e.add_frame(&[1.0; 8], &ex);
    }
    assert_eq!(e.count(0), 10.0);
    // Line 3 is forgotten each time it is out: it holds only the last clean.
    assert_eq!(e.count(3), 1.0);
    assert!((e.dof(0) - 10.0 * dof_per_frame()).abs() < 1e-12);
    assert_eq!(e.dof(3), 2.0);
    // The depth caps a line's frames.
    let mut d = FloorEstimator::new(1, 4);
    for _ in 0..100 {
        d.add_frame(&[2.0], &[false]);
    }
    assert!((d.count(0) - 4.0).abs() < 1e-9 && (d.line(0).unwrap() - 2.0).abs() < 1e-12);
}

/// D6: lines with no clean average carry the error interpolated in dB from
/// the nearest clean lines either side, so a law sees no step there; a run at
/// an end takes the nearest clean line's error.
#[test]
fn unreadable_lines_carry_the_neighbours_interpolated_error() {
    let mut e = FloorEstimator::new(10, 10);
    let target = vec![1.0; 10];
    let mut ex = vec![false; 10];
    for k in [0, 4, 5, 6, 9] {
        ex[k] = true;
    }
    // Measured: +1 dB at line 3, +3 dB at line 7; lines 1, 2, 8 at 0 dB.
    let mut psd = vec![1.0; 10];
    psd[3] = 10f64.powf(0.1);
    psd[7] = 10f64.powf(0.3);
    e.add_frame(&psd, &ex);
    let mut out = vec![0.0; 10];
    let clean = e.estimate(&target, (0, 10), &mut out);
    assert_eq!(clean, 5);
    let db = |v: f64| 10.0 * v.log10();
    assert!((db(out[4]) - 1.5).abs() < 1e-9, "{}", db(out[4]));
    assert!((db(out[5]) - 2.0).abs() < 1e-9);
    assert!((db(out[6]) - 2.5).abs() < 1e-9);
    // The ends take the nearest clean line's error.
    assert!(db(out[0]).abs() < 1e-9 && db(out[9]).abs() < 1e-9);
    assert!((e.min_dof((0, 10)) - 2.0).abs() < 1e-12);
}

#[test]
fn the_mask_covers_a_dwelling_tone_and_a_band_with_its_guard() {
    let mut m = vec![false; 40];
    exclusion_mask(1.0, &[10.0], 2.0, &[(25.0, 30.0)], 2, &mut m);
    let on: Vec<usize> = (0..40).filter(|&k| m[k]).collect();
    // 8..=12 for the tone; 25 and 30 touch the band's bins, so 23..=32.
    assert_eq!(
        on,
        [8, 9, 10, 11, 12, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]
    );
}

/// The guard, measured: a band 20 dB over the floor, synthesized through the
/// drive's own square-root Hann frames and read through Hann frames, spreads
/// into the lines beside its edge lines by +13.7, +4.2, +0.5..0.9, +0.3 dB at
/// one to four lines out. With [`crate::band::BAND_GUARD_LINES`] = 3 left out,
/// the first line the floor reads is within 0.5 dB; at two it would not be.
#[test]
fn the_guard_keeps_a_band_twenty_db_up_out_of_the_floor() {
    let (fs, nfft) = (1024.0, 1024usize);
    let mut psd = vec![0.001; nfft / 2 + 1];
    for p in psd.iter_mut().take(205).skip(195) {
        *p = 0.1;
    }
    let mut synth = ColaSynth::with_seed(nfft, 0.5, fs, 9);
    let mut x = Vec::new();
    while x.len() < (400.0 * fs) as usize {
        x.extend(synth.next_frame(&psd));
    }
    let (_, w) = hertzforge_dsp::spectrum::psd_welch(&x, fs, nfft, 0.5);
    let db = |k: usize| 10.0 * (w[k] / 0.001).log10();
    let out = |o: usize| db(194 + 1 - o).max(db(204 + o));
    let g = crate::band::BAND_GUARD_LINES;
    assert!(out(g + 1) < 0.5, "first read line {:+.2} dB", out(g + 1));
    assert!(out(2) > 1.5, "two lines out {:+.2} dB", out(2));
    let mut m = vec![false; nfft / 2 + 1];
    exclusion_mask(1.0, &[], 0.0, &[(194.5, 204.5)], g, &mut m);
    assert!(m[195 - g] && m[204 + g] && !m[194 - g] && !m[205 + g]);
}

#[test]
fn the_dof_per_frame_is_welchs_for_half_overlapped_hann() {
    assert!((dof_per_frame() - 2.0 / (1.0 + 2.0 / 36.0)).abs() < 1e-12);
    let _ = Gauss::new(1);
}
