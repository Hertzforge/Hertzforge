//! A14 for the synthesis: a drive spectral matrix with a known non-zero
//! cross-spectral phase, played through the generalized drive synthesizer and
//! re-estimated from the time data, comes back as requested; and the synthesized
//! stream carries no frame-rate variance modulation.
use hertzforge_mimo_random::estimate::SdmEstimator;
use hertzforge_random::service::engine::{DriveFactor, DriveSynth};
use hertzforge_random::LoopConfig;
use hertzforge_sdm::{linalg, HermitianStack, C64};

fn cfg() -> LoopConfig {
    let mut c = LoopConfig::demo_default();
    c.fs_hz = 4096.0;
    c.nfft = 512;
    c.overlap = 0.5;
    c.blocks_per_window = 64;
    c
}

/// Two outputs, PSD 1e-3 and 4e-3 V^2/Hz, coherence 0.64, phase +60 degrees
/// (output 0 leads output 1), on lines 20 to 200.
fn request(lines: usize) -> (HermitianStack, (usize, usize), f64, f64) {
    let band = (20, 200);
    let (coh, phase) = (0.64f64, 60f64.to_radians());
    let (a, b) = (1e-3, 4e-3);
    let mut g = HermitianStack::zeros(lines, 2);
    for l in band.0..band.1 {
        g.set(l, 0, 0, C64::new(a, 0.0));
        g.set(l, 1, 1, C64::new(b, 0.0));
        g.set(l, 0, 1, C64::from_polar((coh * a * b).sqrt(), phase));
    }
    (g, band, coh, phase)
}

fn factor(g: &HermitianStack, band: (usize, usize)) -> DriveFactor {
    let l = linalg::sqrt_factor(g).unwrap();
    let mut f = DriveFactor::zeros(g.lines(), 2, 2);
    for line in band.0..band.1 {
        for (d, s) in f.data[line * 4..(line + 1) * 4]
            .iter_mut()
            .zip(l.line(line))
        {
            *d = (s.re, s.im);
        }
    }
    f
}

fn streams(frames: usize) -> (Vec<f64>, Vec<f64>, (usize, usize), f64, f64, HermitianStack) {
    let c = cfg();
    let (g, band, coh, phase) = request(c.nfft / 2 + 1);
    let f = factor(&g, band);
    let mut synth = DriveSynth::with_outputs(&c, 2);
    let (mut a, mut b) = (Vec::new(), Vec::new());
    while a.len() < frames {
        let seg = synth.segment_factored(&f).unwrap();
        a.extend(seg.iter().step_by(2));
        b.extend(seg.iter().skip(1).step_by(2));
    }
    (a, b, band, coh, phase, g)
}

/// A drive SDM re-estimated from the synthesized time data matches the request
/// within its statistical band, phase included. With the synthesizer's or the
/// estimator's conjugation the wrong way round the phase comes back as -60
/// degrees and this fails.
#[test]
fn a14_a_known_cross_phase_survives_synthesis_and_estimation() {
    let c = cfg();
    let (a, b, band, coh, phase, g) = streams(20 * c.window_len());
    let est = SdmEstimator::new(c.nfft, c.fs_hz, 0.5, band);
    let e = est.estimate(&[], 0, &[&a, &b]);
    let k = e.frames as f64;
    let mut worst = (0.0f64, 0.0f64, 0.0f64);
    for l in band.0 + 2..band.1 - 2 {
        for i in 0..2 {
            let r = e.g_dd.get(l, i, i).re / g.get(l, i, i).re;
            worst.0 = worst.0.max((r - 1.0).abs());
        }
        let x = e.g_dd.get(l, 0, 1);
        let gamma2 = x.norm_sqr() / (e.g_dd.get(l, 0, 0).re * e.g_dd.get(l, 1, 1).re);
        worst.1 = worst.1.max((gamma2 - coh).abs());
        let mut dp = x.arg() - phase;
        if dp > std::f64::consts::PI {
            dp -= std::f64::consts::TAU;
        }
        worst.2 = worst.2.max(dp.abs());
    }
    eprintln!(
        "A14 synthesis: {k} frames, worst PSD ratio error {:.4} (bound {:.4}), worst coherence \
         error {:.4}, worst phase error {:.3} deg",
        worst.0,
        4.5 / k.sqrt(),
        worst.1,
        worst.2.to_degrees()
    );
    assert!(
        worst.0 < 4.5 / k.sqrt(),
        "PSD outside the band: {}",
        worst.0
    );
    assert!(worst.1 < 0.03, "coherence: {}", worst.1);
    assert!(
        worst.2 < 3f64.to_radians(),
        "phase: {} deg",
        worst.2.to_degrees()
    );
}

/// The per-frame variance of the synthesized drive shows no frame-rate
/// modulation above 0.2 dB: the variance at each position within a hop, over
/// many hops, smoothed to its first four harmonics, is flat. (A plain Hann
/// synthesis window at 50% overlap would show a 3 dB ripple here.)
#[test]
fn a14_no_frame_rate_variance_modulation() {
    let c = cfg();
    let hop = c.nfft / 2;
    let (a, _, _, _, _, _) = streams(40 * c.window_len());
    let skip = c.nfft; // the stream's first frame ramps up from silence
    let mut prof = vec![0.0; hop];
    let mut n = 0usize;
    for chunk in a[skip..].chunks_exact(hop) {
        for (p, x) in prof.iter_mut().zip(chunk) {
            *p += x * x;
        }
        n += 1;
    }
    for p in prof.iter_mut() {
        *p /= n as f64;
    }
    let mean = prof.iter().sum::<f64>() / hop as f64;
    let mut smooth = vec![mean; hop];
    for h in 1..=4 {
        let (mut cs, mut sn) = (0.0, 0.0);
        for (i, p) in prof.iter().enumerate() {
            let th = std::f64::consts::TAU * (h * i) as f64 / hop as f64;
            cs += p * th.cos();
            sn += p * th.sin();
        }
        let (cs, sn) = (2.0 * cs / hop as f64, 2.0 * sn / hop as f64);
        for (i, s) in smooth.iter_mut().enumerate() {
            let th = std::f64::consts::TAU * (h * i) as f64 / hop as f64;
            *s += cs * th.cos() + sn * th.sin();
        }
    }
    let (lo, hi) = smooth
        .iter()
        .fold((f64::INFINITY, 0.0f64), |(l, h), &v| (l.min(v), h.max(v)));
    let ripple_db = 10.0 * (hi / lo).log10();
    eprintln!("A14 synthesis: frame-rate variance ripple {ripple_db:.3} dB over {n} hops");
    assert!(ripple_db < 0.2, "variance ripple {ripple_db} dB");
}
