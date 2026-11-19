use super::*;
use crate::floor::{hann, periodogram};
use hertzforge_dsp::cola::ColaSynth;

/// The brief's 5.4: at 12 degrees of freedom and 90 % the interval is about
/// -2.3/+3.9 dB. Tabulated chi-square quantiles (5.226, 21.026) give
/// -2.44/+3.61 dB; the brief's figures were rough. Wilson-Hilferty lands
/// within 0.05 dB of the tabulated values.
#[test]
fn the_chi_square_interval_matches_the_tables() {
    let (lo, hi) = chi2_interval_db(12.0, 0.90);
    let (tlo, thi) = (
        -10.0 * (12.0f64 / 21.026).log10(),
        10.0 * (12.0f64 / 5.226).log10(),
    );
    assert!((lo - tlo).abs() < 0.05, "{lo} against {tlo}");
    assert!((hi - thi).abs() < 0.05, "{hi} against {thi}");
    // 120 DOF at 90 %: chi-square 95.70 and 146.57.
    let (lo, hi) = chi2_interval_db(120.0, 0.90);
    assert!((lo - 10.0 * (146.57f64 / 120.0).log10()).abs() < 0.02);
    assert!((hi - 10.0 * (120.0f64 / 95.70).log10()).abs() < 0.02);
    assert!((normal_quantile(0.975) - 1.959_964).abs() < 1e-6);
    // Widening keeps a stated side wider than the scatter.
    assert_eq!(widened_db((3.0, 3.0), 120.0), (3.0, 3.0));
    let w = widened_db((1.0, 1.0), 12.0);
    assert!(w.0 > 2.4 && w.1 > 3.5);
}

#[test]
fn edge_lines_weigh_the_share_of_their_bin_the_band_covers() {
    let mut w = vec![0.0; 10];
    band_weights(0.0, 1.0, 2.25, 5.5, &mut w);
    // Bin 2 is [1.5, 2.5): 0.25 covered; 3, 4, 5 whole; 5 is [4.5, 5.5).
    assert_eq!(w, [0.0, 0.0, 0.25, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
    assert!((w.iter().sum::<f64>() - 3.25).abs() < 1e-12);
    // The inner half of a 10 Hz band at 50 Hz: within 2.5 Hz.
    assert_eq!(inner_lines(0.0, 1.0, 50.0, 10.0, 100), (48, 53));
    // A band narrower than a line still reads the line at its center.
    assert_eq!(inner_lines(0.0, 1.0, 50.2, 0.5, 100), (50, 51));
}

#[test]
fn the_band_servo_converges_under_its_cap_on_a_pure_gain() {
    let mut s = BandServo::default();
    // The plant reads the band at a third of its drive: -4.8 dB.
    let plant = 1.0 / 3.0;
    let mut bound = 0;
    for _ in 0..40 {
        if s.update(plant * s.gain(), 1.0) {
            bound += 1;
        }
    }
    assert!(
        (10.0 * (plant * s.gain()).log10()).abs() < 0.01,
        "{}",
        s.gain_db
    );
    // A 4.8 dB error at half gain asks 2.4 dB: the cap binds the first loops.
    assert!(bound >= 2);
    let mut drive = vec![0.0; 4];
    add_band_drive(
        &[0.0, 0.5, 1.0, 1.0],
        2.0,
        3.0,
        &[1.0, 2.0, 0.0, 4.0],
        &mut drive,
    );
    assert_eq!(drive, [0.0, 1.5, 0.0, 1.5]);
}

/// V4: a 5 Hz band of added level L on a floor of L/100 sweeping 1 Hz/s.
/// Read at a FIXED line, a boxcar average over T seconds centered on the pass
/// gives L min(1, (width/r)/T): L at 1 and 2.5 s, L/2 at 10 s (the closed
/// form); at T = width/r = 5 s the measured share is 0.83, not 1, because the
/// synthesis and analysis frames and the line's own bin round the corners of
/// the pass (the measurement wins; the controller does not read bands at a
/// fixed line). Read in the band's own moving
/// frame (D7), every frame reads L, so the average over the whole sweep is L
/// within its chi-square interval.
#[test]
fn v4_a_swept_band_smears_at_a_fixed_line_and_not_in_its_own_frame() {
    let (fs, nfft) = (256.0, 256usize);
    let df = fs / nfft as f64;
    let (l, width, r, f_start) = (1.0, 5.0, 1.0, 40.0);
    let center = |t: f64| f_start + r * t;
    let secs = 70.0;
    let mut synth = ColaSynth::with_seed(nfft, 0.5, fs, 21);
    let hop = nfft / 2;
    let mut x = Vec::new();
    let mut psd = vec![0.0; nfft / 2 + 1];
    let mut w = vec![0.0; nfft / 2 + 1];
    while x.len() < (secs * fs) as usize {
        let t = (x.len() as f64 + 0.5 * hop as f64) / fs;
        let c = center(t);
        band_weights(0.0, df, c - width / 2.0, c + width / 2.0, &mut w);
        for (p, wk) in psd.iter_mut().zip(&w) {
            *p = l / 100.0 + l * wk;
        }
        psd[0] = 0.0;
        x.extend(synth.next_frame(&psd));
    }
    // Analysis frames.
    let win = hann(nfft);
    let mut scratch = Vec::new();
    let mut frames: Vec<(f64, Vec<f64>)> = Vec::new();
    let mut start = 0;
    while start + nfft <= x.len() {
        let mut p = vec![0.0; nfft / 2 + 1];
        periodogram(&x[start..start + nfft], &win, fs, &mut scratch, &mut p);
        frames.push(((start + nfft / 2) as f64 / fs, p));
        start += hop;
    }
    // Fixed lines 60..=90 Hz, each passed at t = line - 40 s.
    let share_at = |t_avg: f64| -> f64 {
        let mut tot = 0.0;
        let mut n = 0.0;
        for line in 60..=90usize {
            let t0 = (line as f64 * df - f_start) / r;
            let sel: Vec<&Vec<f64>> = frames
                .iter()
                .filter(|(t, _)| (t - t0).abs() <= t_avg / 2.0 + 1e-9)
                .map(|(_, p)| p)
                .collect();
            let m = sel.iter().map(|p| p[line]).sum::<f64>() / sel.len() as f64;
            tot += (m - l / 100.0) / l;
            n += 1.0;
        }
        tot / n
    };
    let s1 = share_at(1.0);
    let s25 = share_at(2.5);
    let s5 = share_at(5.0);
    let s10 = share_at(10.0);
    assert!((s1 - 1.0).abs() < 0.15, "T 1 s: {s1}");
    assert!((s25 - 1.0).abs() < 0.1, "T 2.5 s: {s25}");
    assert!(s5 > 0.75 && s5 < 0.92, "T 5 s: {s5}");
    assert!((s10 - 0.5).abs() < 0.05, "T 10 s: {s10}");
    // The moving frame.
    let mut level = BandLevel::new(0);
    let reference = vec![l / 100.0 + l; nfft / 2 + 1];
    for (t, p) in frames.iter().filter(|(t, _)| *t > 3.0 && *t < secs - 3.0) {
        let lines = inner_lines(0.0, df, center(*t), width, nfft / 2 + 1);
        level.add_frame(p, &reference, lines);
    }
    let (lo, hi) = chi2_interval_db(level.dof(), 0.99);
    let e = level.error_db().unwrap();
    assert!(
        e > -lo && e < hi,
        "moving frame {e:+.2} dB at {} DOF",
        level.dof()
    );
    assert!(e.abs() < 0.5, "{e}");
}
