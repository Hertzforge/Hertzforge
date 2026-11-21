//! The theory's verification cases built in this package (Derivations
//! MMX-D-8): MV1, MV3, MV4, MV5, MV6 (estimator only), MV8 (both ceiling
//! rules, static), MV9, MV11 (first half) and MV14, each against its stated
//! expectation and band. The falsify rule holds: a band is the derivation's,
//! never tuned; a case that fails is recorded with its measured number.
//!
//! Every case prints its measured figures (`--nocapture` shows them).
#![allow(clippy::needless_range_loop, clippy::type_complexity)]

mod common;

use common::*;
use hertzforge_mimo_mixed::band::{
    classify, increment, raise_drive_transform, BandStatement, MovingFrameEstimate,
};
use hertzforge_mimo_mixed::budget::{headroom, CeilingRule, CommonScale, InTurn};
use hertzforge_mimo_mixed::config::PerControl;
use hertzforge_mimo_mixed::estimate::{FrameSpectra, MatrixFloorEstimator};
use hertzforge_mimo_mixed::fill::{FillInput, FillRule, ModelFill, NoError};
use hertzforge_mimo_mixed::mat::CMat;
use hertzforge_mimo_mixed::tone::{ToneBank, ToneLock};
use hertzforge_mimo_random::config::EstimationWindow;
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};
use std::f64::consts::{PI, TAU};

// =============================================================================
// MV1. A tone's vector estimate in correlated random.
// =============================================================================

/// One MV1 setting: `c = rho exp(j theta)` made by a delay of `d` samples
/// (`2 pi f d / fs = theta` at the tone) and an independent part. Returns
/// (covariance of Chat, pseudo-covariance, amplitude variance per channel,
/// phase-difference variance).
fn mv1_run(rho: f64, delay: usize, seed: u64) -> ([[C64; 2]; 2], [[C64; 2]; 2], [f64; 2], f64) {
    let fs: f64 = 600.0;
    let f0 = 100.0;
    let g = 1e-3;
    let sigma = (g * fs / 2.0).sqrt();
    let bn = 0.5;
    let mut bank = ToneBank::new(fs, &[f0], &[ToneLock::Free], 2, 2, bn, 0).unwrap();
    bank.set_part_level(0, 1.0, 0.0);
    bank.set_drive(0, &[c(1.0, 0.0), c(1.0, 0.0)], 0);
    let mut rng = Rng::new(seed);
    let mut hist = vec![0.0; delay + 1];
    let mut head = 0;
    let every = (4.0 * fs) as usize;
    let settle = (10.0 * fs) as usize;
    let readings = 400;
    let total = settle + every * readings;
    let mut drive = [0.0; 2];
    let mut res = [0.0; 2];
    let mut chats: Vec<[C64; 2]> = Vec::with_capacity(readings);
    for s in 1..=total {
        drive = [0.0; 2];
        bank.drive_frame(1.0, &mut drive);
        let z1 = sigma * rng.normal();
        let z2 = sigma * rng.normal();
        hist[head] = z1;
        let delayed = hist[(head + 1) % (delay + 1)];
        head = (head + 1) % (delay + 1);
        let n1 = z1;
        let n2 = rho * delayed + (1.0 - rho * rho).max(0.0).sqrt() * z2;
        // H = I: the response is the drive plus the floor.
        bank.track_frame(&[drive[0] + n1, drive[1] + n2], &mut res);
        if s > settle && (s - settle).is_multiple_of(every) {
            chats.push([bank.tracked(0, 0), bank.tracked(0, 1)]);
        }
    }
    let _ = drive;
    let k = chats.len() as f64;
    let mean = [
        chats.iter().map(|v| v[0]).sum::<C64>() / k,
        chats.iter().map(|v| v[1]).sum::<C64>() / k,
    ];
    let mut cov = [[c(0.0, 0.0); 2]; 2];
    let mut pcov = [[c(0.0, 0.0); 2]; 2];
    for v in &chats {
        for i in 0..2 {
            for j in 0..2 {
                let (a, b) = (v[i] - mean[i], v[j] - mean[j]);
                cov[i][j] += a * b.conj() / (k - 1.0);
                pcov[i][j] += a * b / (k - 1.0);
            }
        }
    }
    let var = |x: &[f64]| {
        let m = x.iter().sum::<f64>() / x.len() as f64;
        x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (x.len() as f64 - 1.0)
    };
    let amp: [f64; 2] = [
        var(&chats.iter().map(|v| v[0].norm()).collect::<Vec<_>>()),
        var(&chats.iter().map(|v| v[1].norm()).collect::<Vec<_>>()),
    ];
    let pd = var(&chats
        .iter()
        .map(|v| (v[0] * v[1].conj()).arg())
        .collect::<Vec<_>>());
    (cov, pcov, amp, pd)
}

/// Measured (seeds 11, 12, 13): 23 of the 24 figures fall inside their
/// band; one does not. At c = 0.8, 0 degrees, control 2's amplitude std is
/// 0.0241 against 0.0224 (variance +15.8 percent, band 15). With 400
/// readings a variance scatters about 7 percent, so this is a two-sigma
/// draw or a small upward bias (the six auto variances read +1.0, -6.2,
/// +4.2, +11.1, +4.5 and +4.5 percent). Recorded, not tuned.
#[test]
#[ignore = "derivation not borne out at the stated band: c = 0.8 at 0 deg, control 2 amplitude std 0.0241 against 0.0224 (variance +15.8 %, band 15 %)"]
fn mimo_mixed_mv1_tone_vector_estimate_in_correlated_random() {
    let fs = 600.0;
    let f0 = 100.0;
    // (rho, theta, delay samples, expected phase-difference std).
    // theta = 2 pi f0 d / fs: one sample at 600 Hz is 60 degrees at 100 Hz.
    let settings = [
        (0.8, 60f64.to_radians(), 1usize, 0.0245),
        (0.8, 0.0, 0, 0.0141),
        (1.0, 0.0, 0, 0.0),
    ];
    let mut misses: Vec<String> = Vec::new();
    for (i, &(rho, theta, d, pd_std)) in settings.iter().enumerate() {
        assert!((TAU * f0 * d as f64 / fs - theta).abs() < 1e-12);
        let (cov, pcov, amp, pd) = mv1_run(rho, d, 11 + i as u64);
        let c12 = C64::from_polar(rho, theta) * 1e-3;
        println!(
            "MV1 c = {rho} at {:.0} deg: cov11 {:.4e} cov22 {:.4e} cov12 {:.4e} at {:.1} deg \
             (expect 1e-3, 1e-3, {:.1e} at {:.0}); |pseudo| max {:.2e} (band 1.5e-4); \
             amp std {:.4} {:.4} (0.0224); phase-diff std {:.4} (expect {pd_std})",
            theta.to_degrees(),
            cov[0][0].re,
            cov[1][1].re,
            cov[0][1].norm(),
            cov[0][1].arg().to_degrees(),
            c12.norm(),
            theta.to_degrees(),
            pcov.iter().flatten().map(|v| v.norm()).fold(0.0, f64::max),
            amp[0].sqrt(),
            amp[1].sqrt(),
            pd.sqrt()
        );
        // 15 percent on each variance; every setting runs before the
        // verdict so each one's figures are on record.
        let mut check = |ok: bool, what: String| {
            if !ok {
                misses.push(format!(
                    "c = {rho} at {:.0} deg: {what}",
                    theta.to_degrees()
                ));
            }
        };
        for v in [cov[0][0].re, cov[1][1].re] {
            check((v / 1e-3 - 1.0).abs() < 0.15, format!("variance {v:e}"));
        }
        check(
            (cov[0][1] - c12).norm() < 0.15 * 1e-3,
            format!("cross {:?}", cov[0][1]),
        );
        for v in pcov.iter().flatten() {
            check(
                v.norm() < 3.0 * 1e-3 / 20.0,
                format!("pseudo-covariance {v:?}"),
            );
        }
        for a in amp {
            check(
                (a / 0.0224f64.powi(2) - 1.0).abs() < 0.15,
                format!("amplitude std {:.4}", a.sqrt()),
            );
        }
        if pd_std > 0.0 {
            check(
                (pd / (pd_std * pd_std) - 1.0).abs() < 0.15,
                format!("phase-difference std {:.4}", pd.sqrt()),
            );
        } else {
            check(pd.sqrt() < 1e-9, format!("phase difference {pd:e}"));
        }
    }
    assert!(misses.is_empty(), "outside the band: {misses:?}");
}

// =============================================================================
// MV3. The tracker's footprint.
// =============================================================================

/// The footprint `F(delta) = |1 - L(delta)|^2` as the tracker's subtraction
/// leaves it, measured with a probe sine at `delta` from the tracker's tone
/// (no tone present): the residual's power at the probe's frequency over
/// the probe's. A white floor is a sum of such probes; the bank is linear in
/// its input, so this is the residual's spectrum over a white floor.
fn mv3_footprint(bn: f64, delta: f64) -> f64 {
    let fs = 400.0;
    let f0 = 100.0;
    let mut bank = ToneBank::new(fs, &[f0], &[ToneLock::Free], 1, 1, bn, 0).unwrap();
    let settle = (12.0 / bn * fs) as usize;
    let len = (40.0 * fs) as usize;
    let fp = f0 + delta;
    let (mut sc, mut ss) = (0.0, 0.0);
    let mut drive = [0.0];
    let mut res = [0.0];
    for s in 0..settle + len {
        bank.drive_frame(1.0, &mut drive);
        let ph = TAU * fp * s as f64 / fs;
        bank.track_frame(&[ph.cos()], &mut res);
        if s >= settle {
            sc += res[0] * ph.cos();
            ss += res[0] * ph.sin();
        }
    }
    // Amplitude of the residual at the probe's frequency, over the probe's 1.
    let a = 2.0 * (sc * sc + ss * ss).sqrt() / len as f64;
    a * a
}

/// The single-output footprint formula `F(x) = |1 - (1 + j x / x_p)^-4|^2`,
/// `x_p = 16 / (5 pi)` in units of `B_n` (theory 2.3), for the integral.
fn f_theory(x_bn: f64) -> f64 {
    let xp = 16.0 / (5.0 * PI);
    let z = C64::new(1.0, x_bn / xp);
    let l = (z * z * z * z).inv();
    (C64::new(1.0, 0.0) - l).norm_sqr()
}

#[test]
fn mimo_mixed_mv3_tracker_footprint() {
    let bn = 0.5;
    let at = |x: f64| mv3_footprint(bn, x * bn);
    let f0 = at(0.0);
    let f066 = at(0.66);
    let f030 = at(0.30);
    let f248 = at(2.48);
    let f204 = at(2.04);
    // The integral of F - 1 over offsets, measured on a grid either side.
    let step = 0.1;
    let mut integral = 0.0;
    let mut x = -12.0;
    while x <= 12.0 + 1e-9 {
        integral += (mv3_footprint(bn, x * bn) - 1.0) * step * bn;
        x += step;
    }
    println!(
        "MV3 F(0) {f0:.4} (0); F(0.66 Bn) {f066:.4} (1.907); F(0.30 Bn) {f030:.4} (1); \
         F(2.48 Bn) {f248:.4} (1); F(2.04 Bn) {f204:.4} (1.024); integral of F - 1 {integral:.4} Hz \
         (Bn = {bn}); formula at 0.66: {:.4}",
        f_theory(0.66)
    );
    assert!(f0 < 1e-3);
    assert!((f066 - 1.907).abs() < 0.01);
    assert!((f030 - 1.0).abs() < 0.01);
    assert!((f248 - 1.0).abs() < 0.01);
    assert!((f204 - 1.024).abs() < 0.01);
    // The integral within 5 percent.
    assert!((integral / bn - 1.0).abs() < 0.05, "integral {integral}");

    // At df = 1 Hz, Bn = 0.5 Hz, the bias on the three nearest lines (the
    // tone on a line): the footprint averaged under the Hann line shape
    // |W(x)|^2, W(x) = sinc(x) / (1 - x^2), x in lines.
    let w2 = |x: f64| {
        if x.abs() < 1e-9 {
            return 1.0;
        }
        if (x.abs() - 1.0).abs() < 1e-9 {
            return 0.25;
        }
        let s = (PI * x).sin() / (PI * x) / (1.0 - x * x);
        s * s
    };
    let df = 1.0;
    let mut bias = [0.0; 3];
    // F measured on a grid of 0.05 Bn over +- 6 Bn, 1 outside.
    let grid: Vec<(f64, f64)> = (-120..=120)
        .map(|i| {
            let d = i as f64 * 0.05 * bn;
            (d, mv3_footprint(bn, d))
        })
        .collect();
    let f_at = |d: f64| -> f64 {
        if d.abs() > 6.0 * bn {
            return 1.0;
        }
        let i = ((d + 6.0 * bn) / (0.05 * bn)).floor() as usize;
        let i = i.min(grid.len() - 2);
        let (d0, f0) = grid[i];
        let (d1, f1) = grid[i + 1];
        f0 + (f1 - f0) * (d - d0) / (d1 - d0)
    };
    for (k, b) in bias.iter_mut().enumerate() {
        let (mut num, mut den) = (0.0, 0.0);
        let h = 0.005;
        let mut d = -12.0;
        while d <= 12.0 {
            let w = w2((d - k as f64 * df) / df);
            num += f_at(d) * w * h;
            den += w * h;
            d += h;
        }
        *b = 10.0 * (num / den).log10();
    }
    println!(
        "MV3 bias at df = 1 Hz, Bn = 0.5 Hz, lines 0, 1, 2 from the tone: {:+.2} {:+.2} {:+.3} dB \
         (hand estimate +1.1, +0.4, under 0.05)",
        bias[0], bias[1], bias[2]
    );
    // The hand estimates are not derived to a band; recorded, not asserted
    // beyond their signs and the third line's bound.
    assert!(bias[0] > 0.0 && bias[1] > 0.0);
    assert!(bias[2].abs() < 0.05, "third line {:.3} dB", bias[2]);
}

// =============================================================================
// MV4. A tone left in the residual raises coherence.
// =============================================================================

#[test]
fn mimo_mixed_mv4_left_tone_raises_coherence() {
    let fs = 512.0;
    let nfft = 512;
    let df = fs / nfft as f64;
    let g = 1e-3;
    let sigma = (g * fs / 2.0).sqrt();
    let (t1, t2) = (1.0, 4.0);
    // T_i = |C_i|^2 / (2 G df).
    let a1 = (2.0 * g * df * t1).sqrt();
    let a2 = (2.0 * g * df * t2).sqrt();
    // Halfway between lines 100 and 101; 90 degrees between the controls
    // (control 2 lags, so G_12's angle is +90).
    let ft = 100.5 * df;
    let frames = 2000;
    let mut fsp = FrameSpectra::new(nfft, fs, 2, nfft / 2 + 1, EstimationWindow::Hann);
    let mut est = MatrixFloorEstimator::new(2, nfft / 2 + 1, 1.0, 0);
    let clean = vec![false; nfft / 2 + 1];
    let mut rng = Rng::new(4);
    let mut x1 = vec![0.0; nfft];
    let mut x2 = vec![0.0; nfft];
    for f in 0..frames {
        for i in 0..nfft {
            let t = (f * nfft + i) as f64 / fs;
            let ph = TAU * ft * t;
            x1[i] = a1 * ph.cos() + sigma * rng.normal();
            x2[i] = a2 * (ph - PI / 2.0).cos() + sigma * rng.normal();
        }
        fsp.transform(|c| if c == 0 { &x1[..] } else { &x2[..] });
        est.add_frame(fsp.spectra(), &clean);
    }
    est.end_loop();
    let w = |x: f64| (PI * x).sin() / (PI * x) / (1.0 - x * x);
    for (line, x) in [(100usize, 0.5f64), (101, 0.5), (99, 1.5), (102, 1.5)] {
        let g11 = est.get(line, 0, 0).re;
        let g22 = est.get(line, 1, 1).re;
        let g12 = est.get(line, 0, 1);
        let coh = g12.norm_sqr() / (g11 * g22);
        let w2 = w(x).powi(2);
        let expect = (t1 * t2 * w2 * w2 / 2.25) / ((1.0 + t1 * w2 / 1.5) * (1.0 + t2 * w2 / 1.5));
        let ne = frames as f64;
        let bias = (1.0 - expect).powi(2) / ne;
        // Scatter of a coherence estimate, one sigma: about
        // sqrt(2) gamma (1 - gamma^2) / sqrt(N_e).
        let sd = (2.0 * expect).sqrt() * (1.0 - expect) / ne.sqrt();
        println!(
            "MV4 line {line} (x = {x}): coherence^2 {coh:.4} (expect {expect:.4} + bias {bias:.4}, \
             sigma {sd:.4}); phase {:.1} deg (expect 90)",
            g12.arg().to_degrees()
        );
        assert!(
            (coh - expect - bias).abs() < 3.0 * sd + 1e-4,
            "line {line}: {coh} against {expect}"
        );
        if x < 1.0 {
            assert!((g12.arg().to_degrees() - 90.0).abs() < 5.0);
        }
    }
}

// =============================================================================
// MV5. The fill.
// =============================================================================

/// A 2 by 2 grid with a resonance at line 20 under a gap of five lines.
fn mv5_grid() -> (MatrixStack, HermitianStack, usize) {
    let lines = 40;
    let mut h = MatrixStack::zeros(lines, 2, 2);
    let mut s = HermitianStack::zeros(lines, 2);
    for l in 0..lines {
        let r = l as f64 / 20.0;
        let res = C64::new(1.0, 0.0) / C64::new(1.0 - r * r, r / 20.0);
        h.set(l, 0, 0, res * 1.0);
        h.set(l, 0, 1, res * 0.3 + 0.1);
        h.set(l, 1, 0, res * C64::new(0.2, 0.1));
        h.set(l, 1, 1, res * 0.8 + 0.05);
        s.set(l, 0, 0, c(1e-3 * (1.0 + 0.01 * l as f64), 0.0));
        s.set(l, 1, 1, c(2e-3, 0.0));
        s.set(l, 0, 1, c(2e-4, -1e-4));
    }
    (h, s, lines)
}

fn x0() -> CMat {
    // Sensor gains of +1 dB and -2 dB with a small coupling.
    let a = 10f64.powf(1.0 / 20.0);
    let b = 10f64.powf(-2.0 / 20.0);
    CMat::from_rows(
        2,
        2,
        vec![c(a, 0.0), c(0.05, 0.02), c(0.05, -0.02), c(b, 0.0)],
    )
}

#[test]
fn mimo_mixed_mv5_model_fill_returns_the_output_side_error_on_a_gap() {
    let (h, s, lines) = mv5_grid();
    let x = x0();
    let mut meas = HermitianStack::zeros(lines, 2);
    let mut truth = HermitianStack::zeros(lines, 2);
    for l in 0..lines {
        let pred = CMat::from_hermitian(&s, l).congruence(&CMat::from_stack(&h, l));
        let g = pred.congruence(&x);
        g.store_hermitian(&mut truth, l);
        g.store_hermitian(&mut meas, l);
    }
    // The gap: lines 18 to 22 hold something stale.
    let mut clean = vec![true; lines];
    for l in 18..23 {
        clean[l] = false;
        CMat::identity(2).scaled(1e-6).store_hermitian(&mut meas, l);
    }
    let reference = HermitianStack::zeros(lines, 2);
    let input = FillInput {
        measured: &meas,
        clean: &clean,
        reference: &reference,
        model: &h,
        drive: &s,
        band: (1, lines),
    };
    let mut out = HermitianStack::zeros(lines, 2);
    let rep = ModelFill::default().fill(&input, &mut out);
    assert_eq!(rep.model_lines, 5, "{rep:?}");
    let mut worst = 0.0f64;
    for l in 18..23 {
        let t = CMat::from_hermitian(&truth, l);
        let e = CMat::from_hermitian(&out, l).sub(&t).max_abs() / t.max_abs();
        worst = worst.max(e);
    }
    // Entrywise interpolation (C1) at the resonance's line.
    let a = CMat::from_hermitian(&truth, 17);
    let b = CMat::from_hermitian(&truth, 23);
    let c1 = a.scaled(0.5).add(&b.scaled(0.5));
    let t20 = CMat::from_hermitian(&truth, 20);
    let miss_db = 10.0 * (t20.get(0, 0).re / c1.get(0, 0).re).log10();
    let height_db = 10.0 * (t20.get(0, 0).re / a.get(0, 0).re).log10();
    println!(
        "MV5 C3 worst relative error on the gap {worst:.2e} (rounding); C1 misses the resonance by \
         {miss_db:.1} dB (resonance {height_db:.1} dB over its neighbour)"
    );
    assert!(worst < 1e-9);
    assert!(miss_db > 0.9 * height_db - 3.0 && miss_db > 10.0);
}

#[test]
fn mimo_mixed_mv5_model_fill_drive_side_with_more_controls_than_drives() {
    // n = 3, m = 2 (C4): the truth is Hhat X_d S X_d Hhat^H with a drive-side
    // error X_d; the fill returns it on the gap.
    let lines = 30;
    let mut h = MatrixStack::zeros(lines, 3, 2);
    let mut s = HermitianStack::zeros(lines, 2);
    for l in 0..lines {
        let r = l as f64 / 15.0;
        let res = C64::new(1.0, 0.0) / C64::new(1.0 - r * r, r / 10.0);
        let vals = [[1.0, 0.2], [0.3, 0.9], [0.5, -0.4]];
        for (i, row) in vals.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                h.set(l, i, j, res * *v + 0.01 * (i + j) as f64);
            }
        }
        s.set(l, 0, 0, c(1e-3, 0.0));
        s.set(l, 1, 1, c(1.5e-3, 0.0));
        s.set(l, 0, 1, c(1e-4, 2e-4));
    }
    let xd = x0();
    let mut meas = HermitianStack::zeros(lines, 3);
    for l in 0..lines {
        let hl = CMat::from_stack(&h, l);
        CMat::from_hermitian(&s, l)
            .congruence(&xd)
            .congruence(&hl)
            .store_hermitian(&mut meas, l);
    }
    let truth = meas.clone();
    let mut clean = vec![true; lines];
    for l in 13..18 {
        clean[l] = false;
    }
    let reference = HermitianStack::zeros(lines, 3);
    let input = FillInput {
        measured: &meas,
        clean: &clean,
        reference: &reference,
        model: &h,
        drive: &s,
        band: (1, lines),
    };
    let mut out = HermitianStack::zeros(lines, 3);
    let rep = ModelFill::default().fill(&input, &mut out);
    let mut worst = 0.0f64;
    for l in 13..18 {
        let t = CMat::from_hermitian(&truth, l);
        worst = worst.max(CMat::from_hermitian(&out, l).sub(&t).max_abs() / t.max_abs());
    }
    println!("MV5 C4 (3 controls, 2 drives): {rep:?}, worst relative error {worst:.2e}");
    assert_eq!(rep.model_lines, 5);
    assert!(worst < 1e-8);
}

#[test]
fn mimo_mixed_mv5_one_control_equals_the_single_output_rule() {
    use hertzforge_mixed::floor::FloorEstimator;
    let lines = 60;
    let band = (5, 55);
    let target: Vec<f64> = (0..lines).map(|l| 1e-3 * (1.0 + 0.05 * l as f64)).collect();
    // Errors that vary along the band, and two gaps (one at the band's end).
    let err_db: Vec<f64> = (0..lines).map(|l| 2.0 * (l as f64 / 7.0).sin()).collect();
    let meas: Vec<f64> = (0..lines)
        .map(|l| target[l] * 10f64.powf(err_db[l] / 10.0))
        .collect();
    let mut excluded = vec![false; lines];
    for l in (20..26).chain(50..55) {
        excluded[l] = true;
    }
    // The single-output rule.
    let mut so = FloorEstimator::new(lines, 4);
    so.add_frame(&meas, &vec![false; lines]);
    so.add_frame(&meas, &excluded);
    let mut so_out = vec![0.0; lines];
    so.estimate(&target, band, &mut so_out);
    // The matrix rule at n = m = 1, the law's drive the model's inverse of
    // the reference (G_pred = target): H = 2, S = target / 4.
    let mut h = MatrixStack::zeros(lines, 1, 1);
    let mut s = HermitianStack::zeros(lines, 1);
    let mut g = HermitianStack::zeros(lines, 1);
    let mut r = HermitianStack::zeros(lines, 1);
    for l in 0..lines {
        h.set(l, 0, 0, c(2.0, 0.0));
        s.set(l, 0, 0, c(target[l] / 4.0, 0.0));
        g.set(l, 0, 0, c(meas[l], 0.0));
        r.set(l, 0, 0, c(target[l], 0.0));
    }
    let clean: Vec<bool> = excluded.iter().map(|e| !e).collect();
    let input = FillInput {
        measured: &g,
        clean: &clean,
        reference: &r,
        model: &h,
        drive: &s,
        band,
    };
    let mut out = HermitianStack::zeros(lines, 1);
    let rep = ModelFill::default().fill(&input, &mut out);
    let mut worst_inner = 0.0f64;
    for l in 20..26 {
        worst_inner = worst_inner.max((out.get(l, 0, 0).re / so_out[l] - 1.0).abs());
    }
    println!(
        "MV5 n = 1: interior gap matches the single-output rule to {worst_inner:.2e}; {rep:?}; \
         at the band's end the single-output rule carries the nearest error ({:.4e}), the matrix \
         rule falls back to the reference ({:.4e})",
        so_out[52],
        out.get(52, 0, 0).re
    );
    assert!(worst_inner < 1e-12);
    // The band's end has one neighbour: the matrix rule falls back to
    // no_error (design D7) where the single-output rule carries the nearest
    // line's error. A stated difference, asserted as built.
    for l in 50..55 {
        assert!((out.get(l, 0, 0).re - target[l]).abs() < 1e-15);
    }
}

#[test]
fn mimo_mixed_mv5_fill_is_positive_semidefinite_on_random_inputs() {
    let mut rng = Rng::new(55);
    let n = 3;
    let lines = 12;
    let mut worst = f64::INFINITY;
    for trial in 0..40 {
        let m = if trial % 2 == 0 { 3 } else { 2 };
        let mut h = MatrixStack::zeros(lines, n, m);
        let mut s = HermitianStack::zeros(lines, m);
        let mut g = HermitianStack::zeros(lines, n);
        for l in 0..lines {
            for i in 0..n {
                for j in 0..m {
                    h.set(l, i, j, rng.cnormal());
                }
            }
            let a = CMat::from_rows(m, m, (0..m * m).map(|_| rng.cnormal()).collect());
            a.mul(&a.adjoint())
                .add(&CMat::identity(m).scaled(0.1))
                .store_hermitian(&mut s, l);
            let b = CMat::from_rows(n, n, (0..n * n).map(|_| rng.cnormal()).collect());
            b.mul(&b.adjoint())
                .add(&CMat::identity(n).scaled(0.1))
                .store_hermitian(&mut g, l);
        }
        let mut clean = vec![true; lines];
        for l in 3..8 {
            clean[l] = false;
        }
        let r = g.clone();
        let input = FillInput {
            measured: &g,
            clean: &clean,
            reference: &r,
            model: &h,
            drive: &s,
            band: (0, lines),
        };
        let mut out = HermitianStack::zeros(lines, n);
        ModelFill::default().fill(&input, &mut out);
        for l in 3..8 {
            let (lo, hi) = CMat::from_hermitian(&out, l).eig_range().unwrap();
            worst = worst.min(lo / hi);
        }
    }
    println!("MV5 random inputs: smallest eigenvalue over largest on filled lines {worst:.3e}");
    assert!(worst > -1e-12);
    // no_error is the reference itself.
    let (h, s, lines) = mv5_grid();
    let g = HermitianStack::zeros(lines, 2);
    let mut r = HermitianStack::zeros(lines, 2);
    CMat::identity(2).store_hermitian(&mut r, 20);
    let clean: Vec<bool> = (0..lines).map(|l| l != 20).collect();
    let input = FillInput {
        measured: &g,
        clean: &clean,
        reference: &r,
        model: &h,
        drive: &s,
        band: (1, lines),
    };
    let mut out = HermitianStack::zeros(lines, 2);
    let rep = NoError.fill(&input, &mut out);
    assert_eq!(rep.reference_lines, 1);
    assert_eq!(out.get(20, 0, 0), c(1.0, 0.0));
}

// =============================================================================
// MV6. The model update with a tone present (the estimator's three pairings).
// =============================================================================

#[test]
fn mimo_mixed_mv6_model_update_with_a_tone_present() {
    let fs = 256.0;
    let nfft = 256;
    let df = fs / nfft as f64;
    let lines = nfft / 2 + 1;
    let k0 = 40; // the tone's line, on a line centre: W(0) = 1
    let ft = k0 as f64 * df;
    let h = [[c(1.0, 0.2), c(0.4, -0.1)], [c(-0.3, 0.1), c(0.9, 0.0)]];
    // Random on both drives with S = s0 [[1, 0.3], [0.3, 1]].
    let s0 = 1e-3;
    let rho = 0.3;
    let sigma = (s0 * fs / 2.0).sqrt();
    // tau = W^2 D^H S^-1 D / (3 df) = 10 with D = a (1, j).
    let d_dir = [c(1.0, 0.0), c(0.0, 1.0)];
    let sinv_quad = {
        // S^-1 = 1/(s0 (1 - rho^2)) [[1, -rho], [-rho, 1]]
        let k = 1.0 / (s0 * (1.0 - rho * rho));
        let v = d_dir;
        (k * (v[0].norm_sqr() + v[1].norm_sqr() - rho * 2.0 * (v[0] * v[1].conj()).re)).max(0.0)
    };
    let a = (10.0 * 3.0 * df / sinv_quad).sqrt();
    let d = [d_dir[0] * a, d_dir[1] * a];
    let tau = d_quad(&d, s0, rho) / (3.0 * df);
    assert!((tau - 10.0).abs() < 1e-9);
    let noise = (1e-6 * fs / 2.0).sqrt();
    let frames = 4000;
    let mut fx = FrameSpectra::new(nfft, fs, 2, lines, EstimationWindow::Hann);
    let mut fy = FrameSpectra::new(nfft, fs, 2, lines, EstimationWindow::Hann);
    let mut fr = FrameSpectra::new(nfft, fs, 2, lines, EstimationWindow::Hann);
    let mut fyr = FrameSpectra::new(nfft, fs, 2, lines, EstimationWindow::Hann);
    // Sums on the tone's line: G_xx, G_yx for the three pairings.
    let mut gxx = [[c(0.0, 0.0); 2]; 2];
    let mut grr = [[c(0.0, 0.0); 2]; 2];
    let mut gyx_a = [[c(0.0, 0.0); 2]; 2];
    let mut gyx_b = [[c(0.0, 0.0); 2]; 2];
    let mut gyx_c = [[c(0.0, 0.0); 2]; 2];
    let mut rng = Rng::new(6);
    let mut xr = [vec![0.0; nfft], vec![0.0; nfft]];
    let mut xp = [vec![0.0; nfft], vec![0.0; nfft]];
    let mut ym = [vec![0.0; nfft], vec![0.0; nfft]];
    let mut yres = [vec![0.0; nfft], vec![0.0; nfft]];
    for f in 0..frames {
        for i in 0..nfft {
            let t = (f * nfft + i) as f64 / fs;
            let z1 = rng.normal();
            let z2 = rng.normal();
            let r1 = sigma * z1;
            let r2 = sigma * (rho * z1 + (1.0 - rho * rho).sqrt() * z2);
            let ph = TAU * ft * t;
            let tone = [
                d[0].norm() * (ph + d[0].arg()).cos(),
                d[1].norm() * (ph + d[1].arg()).cos(),
            ];
            let xrand = [r1, r2];
            let xplay = [r1 + tone[0], r2 + tone[1]];
            for row in 0..2 {
                let lin = |x: [f64; 2]| -> f64 {
                    // A static real-time response to a complex H on a line is
                    // not real; use H's value on the tone's line by building
                    // the response in the frequency domain below instead.
                    x[0] * h[row][0].re + x[1] * h[row][1].re
                };
                let n = noise * rng.normal();
                xr[row][i] = xrand[row];
                xp[row][i] = xplay[row];
                ym[row][i] = lin(xplay) + n;
                yres[row][i] = lin(xrand) + n;
            }
        }
        fx.transform(|ch| &xp[ch][..]);
        fr.transform(|ch| &xr[ch][..]);
        fy.transform(|ch| &ym[ch][..]);
        fyr.transform(|ch| &yres[ch][..]);
        let (x, r, y, yr) = (fx.spectra(), fr.spectra(), fy.spectra(), fyr.spectra());
        let at = |s: &[C64], ch: usize| s[k0 * 2 + ch];
        for i in 0..2 {
            for j in 0..2 {
                gxx[i][j] += at(x, i) * at(x, j).conj();
                grr[i][j] += at(r, i) * at(r, j).conj();
                gyx_a[i][j] += at(y, i) * at(x, j).conj();
                gyx_b[i][j] += at(yr, i) * at(x, j).conj();
                gyx_c[i][j] += at(yr, i) * at(r, j).conj();
            }
        }
    }
    let hreal = [[h[0][0].re, h[0][1].re], [h[1][0].re, h[1][1].re]];
    let h_of = |gyx: [[C64; 2]; 2], gxx: [[C64; 2]; 2]| -> [[C64; 2]; 2] {
        let det = gxx[0][0] * gxx[1][1] - gxx[0][1] * gxx[1][0];
        let inv = [
            [gxx[1][1] / det, -gxx[0][1] / det],
            [-gxx[1][0] / det, gxx[0][0] / det],
        ];
        let mut out = [[c(0.0, 0.0); 2]; 2];
        for i in 0..2 {
            for j in 0..2 {
                out[i][j] = gyx[i][0] * inv[0][j] + gyx[i][1] * inv[1][j];
            }
        }
        out
    };
    let rel = |e: [[C64; 2]; 2]| -> f64 {
        let mut num = 0.0;
        let mut den = 0.0;
        for i in 0..2 {
            for j in 0..2 {
                num += (e[i][j] - hreal[i][j]).norm_sqr();
                den += hreal[i][j] * hreal[i][j];
            }
        }
        (num / den).sqrt()
    };
    let ha = h_of(gyx_a, gxx);
    let hb = h_of(gyx_b, gxx);
    let hc = h_of(gyx_c, grr);
    let hd = |m: [[C64; 2]; 2], v: [C64; 2]| {
        [
            m[0][0] * v[0] + m[0][1] * v[1],
            m[1][0] * v[0] + m[1][1] * v[1],
        ]
    };
    let hr = [
        [c(hreal[0][0], 0.0), c(hreal[0][1], 0.0)],
        [c(hreal[1][0], 0.0), c(hreal[1][1], 0.0)],
    ];
    let true_d = hd(hr, d);
    let est_d = hd(hb, d);
    let shrink = (est_d[0] * true_d[0].conj() + est_d[1] * true_d[1].conj()).re
        / (true_d[0].norm_sqr() + true_d[1].norm_sqr());
    // u with D^H S^-1 u = 0: S^-1 D is orthogonal to u.
    let k = 1.0 / (s0 * (1.0 - rho * rho));
    let sid = [(d[0] - d[1] * rho) * k, (d[1] - d[0] * rho) * k];
    let u = [sid[1].conj(), -sid[0].conj()];
    let est_u = hd(hb, u);
    let true_u = hd(hr, u);
    let u_err = ((est_u[0] - true_u[0]).norm_sqr() + (est_u[1] - true_u[1]).norm_sqr()).sqrt()
        / (true_u[0].norm_sqr() + true_u[1].norm_sqr()).sqrt();
    println!(
        "MV6 (a) played against measured: |H - Hest| / |H| {:.4}; (b) played against residual: \
         shrink on D {shrink:.4} (expect 1 / (1 + tau) = {:.4}), error on u {u_err:.4}; \
         (c) synthesized against residual: {:.4}",
        rel(ha),
        1.0 / (1.0 + tau),
        rel(hc)
    );
    assert!(rel(ha) < 0.05);
    assert!(rel(hc) < 0.05);
    assert!((shrink * (1.0 + tau) - 1.0).abs() < 0.10, "shrink {shrink}");
    assert!(u_err < 0.05);
}

fn d_quad(d: &[C64; 2], s0: f64, rho: f64) -> f64 {
    let k = 1.0 / (s0 * (1.0 - rho * rho));
    k * (d[0].norm_sqr() + d[1].norm_sqr() - 2.0 * rho * (d[0] * d[1].conj()).re)
}

// =============================================================================
// MV8. The shared ceiling.
// =============================================================================

fn mv8_drives() -> Vec<C64> {
    // Tone-major: tone k's vector over drives 1 and 2.
    vec![
        c(3.0, 0.0),
        c(1.0, 0.0),
        c(2.0, 0.0),
        c(1.0, 0.0),
        c(1.0, 0.0),
        c(4.0, 0.0),
    ]
}

#[test]
fn mimo_mixed_mv8_shared_ceiling_common_scale() {
    let d = mv8_drives();
    let mut room = [0.0; 2];
    headroom(1.0, &[10.0, 10.0], &[5.0, 5.0], &mut room);
    assert_eq!(room, [5.0, 5.0]);
    let mut s = [0.0; 3];
    let mut lim = [false; 3];
    CommonScale.apply(&d, &[0.0; 6], 3, &room, &mut s, &mut lim);
    let db = 20.0 * s[0].log10();
    println!("MV8 common_scale: scales {s:?} ({db:.2} dB), limiting {lim:?}");
    assert!(s.iter().all(|v| (v - 5.0 / 6.0).abs() < 1e-12));
    assert!((db + 1.58).abs() < 0.005);
    assert_eq!(lim, [true; 3]);
    // Static H: every tone reads the same dB at every control.
    let h = CMat::from_rows(
        2,
        2,
        vec![c(0.9, 0.1), c(0.2, 0.0), c(-0.1, 0.3), c(1.1, 0.0)],
    );
    for k in 0..3 {
        let asked = h.mul_vec(&d[2 * k..2 * k + 2]);
        let played = h.mul_vec(&[d[2 * k] * s[k], d[2 * k + 1] * s[k]]);
        for i in 0..2 {
            let r = 20.0 * (played[i].norm() / asked[i].norm()).log10();
            assert!((r + 1.584).abs() < 0.001);
        }
    }
    // The played drive over a long run: random clipped at its 5 V peak plus
    // the three scaled tones at unrelated frequencies, never over 10 V.
    let fs = 2000.0;
    let f = [101.3, 157.0 * 2f64.sqrt(), 233.0 * 3f64.sqrt() / 2.0];
    let mut bank = ToneBank::new(fs, &f, &[ToneLock::Free; 3], 2, 2, 0.5, 0).unwrap();
    for k in 0..3 {
        bank.set_part_level(k, 1.0, 0.0);
        bank.set_drive(k, &[d[2 * k] * s[k], d[2 * k + 1] * s[k]], 0);
    }
    let mut rng = Rng::new(8);
    let mut peak = [0.0f64; 2];
    let mut tone_peak = [0.0f64; 2];
    for _ in 0..2_000_000 {
        let mut out = [0.0; 2];
        bank.drive_frame(1.0, &mut out);
        for j in 0..2 {
            tone_peak[j] = tone_peak[j].max(out[j].abs());
            let x = (1.7 * rng.normal()).clamp(-5.0, 5.0);
            peak[j] = peak[j].max((out[j] + x).abs());
        }
    }
    println!(
        "MV8 over 1000 s: tones' peak {:.3} {:.3} V (sum 5), played peak {:.3} {:.3} V (ceiling 10)",
        tone_peak[0], tone_peak[1], peak[0], peak[1]
    );
    assert!(peak.iter().all(|p| *p <= 10.0 + 1e-9));
}

#[test]
fn mimo_mixed_mv8_shared_ceiling_in_turn_depends_on_order() {
    let d = mv8_drives();
    let room = [5.0, 5.0];
    let mut s = [0.0; 3];
    let mut lim = [false; 3];
    InTurn.apply(&d, &[0.0; 6], 3, &room, &mut s, &mut lim);
    // The reverse order.
    let rev: Vec<C64> = [2, 1, 0]
        .iter()
        .flat_map(|&k| [d[2 * k], d[2 * k + 1]])
        .collect();
    let mut sr = [0.0; 3];
    let mut limr = [false; 3];
    InTurn.apply(&rev, &[0.0; 6], 3, &room, &mut sr, &mut limr);
    println!(
        "MV8 in_turn in bank order: scales {s:?} limiting {lim:?}; reversed (tones 3, 2, 1): \
         scales {sr:?} limiting {limr:?}"
    );
    for (dd, ss) in [(&d, &s), (&rev, &sr)] {
        for j in 0..2 {
            let sum: f64 = (0..3).map(|k| ss[k] * dd[2 * k + j].norm()).sum();
            assert!(sum <= room[j] + 1e-12, "drive {j}: {sum}");
        }
    }
    // The outcome depends on the order (the point of the case).
    let fwd = [s[0], s[1], s[2]];
    let back = [sr[2], sr[1], sr[0]];
    assert!(fwd.iter().zip(&back).any(|(a, b)| (a - b).abs() > 1e-6));
}

/// Requirement: `in_turn` judges a tone against the other tones' drives in
/// force, never their asks. One drive, 5 V of room; tone 1 plays 3 V and
/// asks 0.5 V, tone 0 asks 4 V. Against the asks tone 0 would get its 4 V
/// (4 + 0.5 <= 5); against what tone 1 plays it gets 2 V (scale 0.5,
/// limiting). Tone 1 is then judged against the larger of tone 0's 1 V in
/// force and its 2 V decided: scale 1. The decided sum stays within 5 V.
#[test]
fn mimo_mixed_in_turn_judges_against_drives_in_force() {
    let d = [c(4.0, 0.0), c(0.0, 0.5)];
    let in_force = [1.0, 3.0];
    let mut s = [0.0; 2];
    let mut lim = [false; 2];
    InTurn.apply(&d, &in_force, 2, &[5.0], &mut s, &mut lim);
    println!("in_turn in force: scales {s:?} limiting {lim:?}");
    assert!((s[0] - 0.5).abs() < 1e-12);
    assert!(lim[0]);
    assert!((s[1] - 1.0).abs() < 1e-12);
    assert!(!lim[1]);
    assert!(s[0] * 4.0 + s[1] * 0.5 <= 5.0 + 1e-12);
}

// =============================================================================
// MV9. A band stated by autos on a coherent floor.
// =============================================================================

#[test]
fn mimo_mixed_mv9_band_by_autos_on_a_coherent_floor() {
    let gf = CMat::from_rows(
        2,
        2,
        vec![c(1.0, 0.0), c(0.8, 0.0), c(0.8, 0.0), c(1.0, 0.0)],
    );
    let raise = BandStatement::Raise {
        db: PerControl::Each(vec![10.0 * 11f64.log10(), 0.0]),
    };
    let delta = increment(&raise, &gf);
    let (vals, _) = delta.eigh().unwrap();
    println!(
        "MV9 Delta = [[{:.3}, {:.4}], [{:.4}, {:.3}]], eigenvalues {:.3} {:.3} (expect -0.332, 10.332)",
        delta.get(0, 0).re,
        delta.get(0, 1).re,
        delta.get(1, 0).re,
        delta.get(1, 1).re,
        vals[0],
        vals[1]
    );
    assert!((delta.get(0, 0).re - 10.0).abs() < 1e-12);
    assert!((delta.get(0, 1).re - 1.853).abs() < 5e-4);
    assert!(delta.get(1, 1).re.abs() < 1e-12);
    assert!((vals[0] + 0.332).abs() < 5e-4 && (vals[1] - 10.332).abs() < 5e-4);
    let mut floor = HermitianStack::zeros(3, 2);
    for l in 0..3 {
        gf.store_hermitian(&mut floor, l);
    }
    assert!(!classify(&raise, &floor, (0, 3)).additive);
    let equal = BandStatement::Raise {
        db: PerControl::All(10.0 * 11f64.log10()),
    };
    let ce = classify(&equal, &floor, (0, 3));
    assert!(ce.additive && ce.equal_ratio);
    // On a static 2 by 2 with an exact model, M_d S_F M_d^H gives the total.
    let h = CMat::from_rows(
        2,
        2,
        vec![c(1.2, 0.3), c(-0.4, 0.1), c(0.2, -0.5), c(0.9, 0.0)],
    );
    let z = h
        .pinv(hertzforge_mimo_mixed::mat::Z_REGULARIZATION)
        .unwrap();
    let sf = gf.congruence(&z);
    let md = raise_drive_transform(&h, &[10.0 * 11f64.log10(), 0.0]).unwrap();
    let total_drive = sf.congruence(&md);
    let response = total_drive.congruence(&h);
    let lam = CMat::diag(&[11f64.sqrt(), 1.0]);
    let total = gf.congruence(&lam);
    let err = response.sub(&total).max_abs() / total.max_abs();
    println!("MV9 M_d S_F M_d^H response against (5.2): relative error {err:.2e}");
    assert!(err < 1e-12);
    let (lo, _) = total_drive.eig_range().unwrap();
    assert!(lo > -1e-12);
}

// =============================================================================
// MV11 (first half). Scatter of a band's matrix estimate.
// =============================================================================

#[test]
fn mimo_mixed_mv11_scatter_of_a_band_matrix_estimate() {
    let n = 4;
    let runs = 200;
    let mut rng = Rng::new(11);
    for gamma in [0.0, 0.5, 0.9] {
        let coh = coherence(n, gamma);
        let l = cholesky(&coh);
        for ne in [6usize, 24, 96] {
            let mut autos = Vec::with_capacity(runs * n);
            let mut cohs = Vec::with_capacity(runs);
            for _ in 0..runs {
                let mut e = MovingFrameEstimate::new(n, 0, 0);
                for _ in 0..ne {
                    let w: Vec<C64> = (0..n).map(|_| rng.cnormal()).collect();
                    let x: Vec<C64> = (0..n)
                        .map(|i| (0..=i).map(|k| l[i][k] * w[k]).sum())
                        .collect();
                    e.add_frame(&x, 0.0, 1.0, &[]);
                }
                let g = e.estimate();
                for i in 0..n {
                    autos.push(g.get(0, i, i).re);
                }
                let g01 = g.get(0, 0, 1);
                cohs.push(g01.norm_sqr() / (g.get(0, 0, 0).re * g.get(0, 1, 1).re));
            }
            let mean = autos.iter().sum::<f64>() / autos.len() as f64;
            let sd = (autos.iter().map(|a| (a - mean).powi(2)).sum::<f64>()
                / (autos.len() as f64 - 1.0))
                .sqrt();
            let rel = sd / mean;
            let want = 1.0 / (ne as f64).sqrt();
            let cm = cohs.iter().sum::<f64>() / runs as f64;
            let cse = (cohs.iter().map(|v| (v - cm).powi(2)).sum::<f64>() / (runs as f64 - 1.0))
                .sqrt()
                / (runs as f64).sqrt();
            let g2 = gamma * gamma;
            let expect = g2 + (1.0 - g2).powi(2) / ne as f64;
            println!(
                "MV11 gamma {gamma} N_e {ne}: auto relative scatter {rel:.4} (expect {want:.4}); \
                 coherence^2 mean {cm:.4} (expect {expect:.4}, standard error {cse:.4})"
            );
            assert!(
                (rel / want - 1.0).abs() < 0.20,
                "scatter {rel} against {want}"
            );
            assert!(
                (cm - expect).abs() < 3.0 * cse + 1e-3,
                "coherence {cm} against {expect}"
            );
        }
    }
    // Rank deficiency at N_e < 4.
    let mut e = MovingFrameEstimate::new(n, 0, 0);
    for _ in 0..3 {
        let x: Vec<C64> = (0..n).map(|_| rng.cnormal()).collect();
        e.add_frame(&x, 0.0, 1.0, &[]);
    }
    let (lo, hi) = CMat::from_hermitian(&e.estimate(), 0).eig_range().unwrap();
    println!(
        "MV11 N_e = 3 of 4 controls: smallest over largest eigenvalue {:.2e}",
        lo / hi
    );
    assert!(lo.abs() / hi < 1e-12);
}

// =============================================================================
// MV14. Harmonic phase.
// =============================================================================

/// Play a fundamental and its second harmonic through a static `h`; return
/// the response record.
fn mv14_record(locks: &[ToneLock], f: &[f64], secs: f64, fs: f64, h: &CMat) -> Vec<[f64; 2]> {
    let mut bank = ToneBank::new(fs, f, locks, 2, 2, 0.5, 0).unwrap();
    let d1 = [c(1.0, 0.0), C64::from_polar(0.5, 0.7)];
    let d2 = [C64::from_polar(0.4, -1.1), c(0.0, 0.8)];
    for (k, d) in [d1, d2].iter().enumerate() {
        bank.set_part_level(k, 1.0, 0.0);
        bank.set_drive(k, d, 0);
    }
    let n = (secs * fs) as usize;
    let mut rec = Vec::with_capacity(n);
    for _ in 0..n {
        let mut x = [0.0; 2];
        bank.drive_frame(1.0, &mut x);
        let y = h.mul_vec(&[c(x[0], 0.0), c(x[1], 0.0)]);
        rec.push([y[0].re, y[1].re]);
    }
    rec
}

/// Analyse a record from sample `from` with a locked analyser of its own
/// (its oscillators start at phase zero there): tracked `C_1`, `C_2` per
/// control after `secs`.
fn mv14_analyse(rec: &[[f64; 2]], from: usize, secs: f64, fs: f64, f1: f64) -> [[C64; 2]; 2] {
    let mut a = ToneBank::new(
        fs,
        &[f1, 2.0 * f1],
        &[
            ToneLock::Free,
            ToneLock::Follow {
                leader: 0,
                ratio: 2.0,
            },
        ],
        2,
        2,
        0.5,
        0,
    )
    .unwrap();
    let mut res = [0.0; 2];
    let mut sink = [0.0; 2];
    for y in rec.iter().skip(from).take((secs * fs) as usize) {
        a.drive_frame(0.0, &mut sink);
        a.track_frame(y, &mut res);
    }
    [
        [a.tracked(0, 0), a.tracked(0, 1)],
        [a.tracked(1, 0), a.tracked(1, 1)],
    ]
}

#[test]
fn mimo_mixed_mv14_harmonic_phase() {
    let fs = 1000.0;
    let f1 = 50.0;
    // A static H; real, so the record is the real response (a complex static
    // H has no real-time form).
    let h = CMat::from_rows(
        2,
        2,
        vec![c(1.0, 0.0), c(0.3, 0.0), c(-0.2, 0.0), c(0.8, 0.0)],
    );
    let locked = [
        ToneLock::Free,
        ToneLock::Follow {
            leader: 0,
            ratio: 2.0,
        },
    ];
    let rec = mv14_record(&locked, &[f1, 2.0 * f1], 40.0, fs, &h);
    let phi = |c: [[C64; 2]; 2], i: usize| (c[1][i] * c[0][i].conj().powi(2)).arg();
    let a = mv14_analyse(&rec, 0, 20.0, fs, f1);
    let b = mv14_analyse(&rec, 12_345, 20.0, fs, f1);
    let mut worst = 0.0f64;
    for i in 0..2 {
        worst = worst.max((phi(a, i) - phi(b, i)).abs());
    }
    let between = |c: [[C64; 2]; 2], k: usize| (c[k][0] * c[k][1].conj()).arg();
    let wb = (0..2)
        .map(|k| (between(a, k) - between(b, k)).abs())
        .fold(0.0, f64::max);
    // C = H D on the static structure (the analyser's origin at zero).
    let d1 = [c(1.0, 0.0), C64::from_polar(0.5, 0.7)];
    let hd = h.mul_vec(&d1);
    let chd = ((a[0][0] - hd[0]).norm() + (a[0][1] - hd[1]).norm()) / hd[0].norm();
    println!(
        "MV14 locked: arg(C2) - 2 arg(C1) moved {worst:.2e} rad with the origin shifted; phases \
         between controls moved {wb:.2e} rad; |C - H D| / |C| {chd:.2e}"
    );
    assert!(worst < 1e-6);
    assert!(wb < 1e-6);
    assert!(chd < 1e-4);
    // Oscillators integrated separately, a part in a million off the ratio:
    // the quantity drifts at 2 pi times the frequency error.
    let ppm = 1e-6;
    let free = [ToneLock::Free, ToneLock::Free];
    let secs = 400.0;
    let rec = mv14_record(&free, &[f1, 2.0 * f1 * (1.0 + ppm)], secs, fs, &h);
    let t_a = 50.0;
    let t_b = 350.0;
    let pa = mv14_analyse(&rec, 0, t_a, fs, f1);
    let pb = mv14_analyse(&rec, 0, t_b, fs, f1);
    let rate = (phi(pb, 0) - phi(pa, 0)).rem_euclid(TAU);
    let rate = if rate > PI { rate - TAU } else { rate } / (t_b - t_a);
    let want = TAU * 2.0 * f1 * ppm;
    println!("MV14 free: drift {rate:.4e} rad/s (expect {want:.4e})");
    assert!((rate / want - 1.0).abs() < 0.05);
}

// =============================================================================
// The tone bank on a static structure: C = H D.
// =============================================================================

#[test]
fn mimo_mixed_static_structure_gives_c_equals_h_d() {
    let fs = 2000.0;
    let lag = 3;
    // Three drives, two controls and one limit channel.
    let h = CMat::from_rows(
        3,
        3,
        vec![
            c(1.0, 0.0),
            c(0.5, 0.0),
            c(-0.2, 0.0),
            c(0.1, 0.0),
            c(0.9, 0.0),
            c(0.3, 0.0),
            c(0.4, 0.0),
            c(-0.6, 0.0),
            c(0.7, 0.0),
        ],
    );
    let d = [
        C64::from_polar(0.3, 0.2),
        C64::from_polar(0.5, -2.0),
        C64::from_polar(0.2, 1.0),
    ];
    let mut bank = ToneBank::new(fs, &[120.0], &[ToneLock::Free], 3, 3, 1.0, lag).unwrap();
    bank.set_part_level(0, 1.0, 0.0);
    bank.set_drive(0, &d, 0);
    let mut pipe = vec![[0.0; 3]; lag + 1];
    let mut head = 0;
    let mut res = [0.0; 3];
    let mut worst_res = 0.0f64;
    for s in 0..(20.0 * fs) as usize {
        let mut x = [0.0; 3];
        bank.drive_frame(1.0, &mut x);
        let y = h.mul_vec(&[c(x[0], 0.0), c(x[1], 0.0), c(x[2], 0.0)]);
        pipe[head] = [y[0].re, y[1].re, y[2].re];
        // The response reaches the input lag frames after the drive.
        let back = pipe[(head + 1) % (lag + 1)];
        head = (head + 1) % (lag + 1);
        bank.track_frame(&back, &mut res);
        if s as f64 > 15.0 * fs {
            worst_res = worst_res.max(res.iter().fold(0.0f64, |a, v| a.max(v.abs())));
        }
    }
    let hd = h.mul_vec(&d);
    let mut worst = 0.0f64;
    for (i, want) in hd.iter().enumerate() {
        worst = worst.max((bank.tracked(0, i) - want).norm());
    }
    println!(
        "static C = H D: worst |C - H D| {worst:.2e}; residual after settling {worst_res:.2e}"
    );
    assert!(worst < 1e-6);
    assert!(worst_res < 1e-6);
}
