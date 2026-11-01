//! Emit eigen-dsp outputs for a fixed set of cases as machine-readable lines,
//! so the independent `tools/dsp-xref/xref.py` harness can diff the Rust core
//! against a numpy/scipy reference it computes itself.
//!
//! Output format: one `key=csv` line per case on stdout. No dependencies; the
//! harness parses it with the standard library. Cases and their parameters are
//! duplicated verbatim in xref.py so the two sides describe the same experiment.
//!
//! Run: `cargo run -p eigen-dsp --example xref_dump`

use eigen_dsp::{fft, integrate, spectrum, srs};
use std::f64::consts::TAU;

fn print_vec(key: &str, v: &[f64]) {
    let body: Vec<String> = v.iter().map(|x| format!("{x:.10e}")).collect();
    println!("{key}={}", body.join(","));
}

fn main() {
    // ---- Case FFT: magnitude spectrum of a pure cosine at bin 17, N=256 ----
    let n = 256usize;
    let m = 17usize;
    let x: Vec<f64> = (0..n)
        .map(|k| (TAU * m as f64 * k as f64 / n as f64).cos())
        .collect();
    let spec = fft::fft(&fft::zero_pad_pow2(&x));
    let mags: Vec<f64> = spec.iter().map(|&c| fft::mag(c)).collect();
    println!("fft_n={n}");
    println!("fft_bin={m}");
    print_vec("fft_mag", &mags);

    // ---- Case PSD: unit sine, fs=1024, f0=64, nfft=4096, 50% overlap ----
    let fs = 1024.0;
    let f0 = 64.0;
    let npsd = 1 << 16;
    let sig: Vec<f64> = (0..npsd)
        .map(|k| (TAU * f0 * k as f64 / fs).sin())
        .collect();
    let nfft = 4096usize;
    let (freqs, psd) = spectrum::psd_welch(&sig, fs, nfft, 0.5);
    println!("psd_fs={fs}");
    println!("psd_f0={f0}");
    println!("psd_nfft={nfft}");
    println!("psd_overlap=0.5");
    // dump a handful of bins around the peak plus the integral, to keep the line short
    let df = fs / nfft as f64;
    let peak_bin = (f0 / df).round() as usize;
    let lo = peak_bin.saturating_sub(3);
    let hi = (peak_bin + 4).min(psd.len());
    print_vec("psd_freqs_window", &freqs[lo..hi]);
    print_vec("psd_window", &psd[lo..hi]);
    let area: f64 = psd.iter().sum::<f64>() * df;
    println!("psd_integral={area:.10e}");

    // ---- Case SRS: half-sine pulse, amp=10 g, tau=5 ms, fs=20 kHz, Q=10 ----
    let srs_fs = 20_000.0_f64;
    let amp = 10.0_f64;
    let tau = 0.005_f64;
    let n_pulse = (tau * srs_fs).round() as usize;
    let n_quiet = (0.1 * srs_fs).round() as usize;
    let mut pulse = Vec::with_capacity(n_pulse + n_quiet);
    for k in 0..n_pulse {
        let t = k as f64 / srs_fs;
        pulse.push(amp * (std::f64::consts::PI * t / tau).sin());
    }
    pulse.extend(std::iter::repeat_n(0.0, n_quiet));
    let srs_freqs: Vec<f64> = vec![
        10.0, 20.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0,
    ];
    let q = 10.0;
    let srs_out = srs::srs_analyze(&pulse, srs_fs, &srs_freqs, q);
    println!("srs_fs={srs_fs}");
    println!("srs_amp={amp}");
    println!("srs_tau={tau}");
    println!("srs_q={q}");
    print_vec("srs_freqs", &srs_freqs);
    print_vec("srs_out", &srs_out);

    // ---- Case integrate: sine 2g at 100 Hz, fs=10 kHz, HP=2 Hz ----
    let ifs = 10_000.0;
    let ifreq = 100.0;
    let iamp = 2.0;
    let w = TAU * ifreq;
    let nn = 40_000usize;
    let accel: Vec<f64> = (0..nn).map(|k| iamp * (w * k as f64 / ifs).sin()).collect();
    let hp = 2.0;
    let vel = integrate::accel_to_velocity(&accel, ifs, hp);
    println!("int_fs={ifs}");
    println!("int_freq={ifreq}");
    println!("int_amp={iamp}");
    println!("int_hp={hp}");
    println!("int_g_to_in_s2={:.10e}", integrate::G_TO_IN_S2);
    // settled-tail amplitude
    let tail = &vel[nn - (ifs as usize)..];
    let vmax = tail.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
    println!("int_vel_amp={vmax:.10e}");
}
