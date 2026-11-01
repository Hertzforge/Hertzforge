//! Gate tests for the analysis pipeline (`analysis.rs`):
//!
//! * EU taxonomy goldens (EUpk / EUrms / autopower on a pure tone; flattop
//!   amplitude accuracy) and the cross-window PSD invariance.
//! * PSD level on seeded known-variance noise, Parseval, and the
//!   `autopower = psd * df * enbw` identity.
//! * FRF: H1 against an ANALYTIC biquad plant (magnitude + phase, matching
//!   the pinned eigen-dsp `Gxy = <X*conj(Y)>` convention), the
//!   `|H1| = gamma * |Hv|` identity and output-noise bias, the
//!   single-average coherence trap AS a test, transmissibility.
//! * Time stats: sine crest sqrt(2), sine kurtosis 1.5, seeded-Gaussian
//!   kurtosis ~3 in the vibration convention.
//! * Decimated end-to-end tone through the DecimatorCascade.
//! * Live-vs-offline equivalence through a real recording round-trip,
//!   for both the spectrum path and the FRF path.
//! * `integrate_region` settle-transient trim.
//!
//! Every reference is analytic or independently computed in-test (the biquad
//! reference response is evaluated in closed form, not via the crate's FFT).
//! All noise is seeded xorshift64 - fully deterministic.

use std::path::PathBuf;

use eigen_channel_table::{Channel, ChannelTable};
use eigen_recorder::{
    analyze_region, analyze_region_frf, hop_frames, integrate_region, psd_accel_to_displacement,
    psd_accel_to_velocity, psd_velocity_to_displacement, time_stats, AcquisitionConfig,
    AveragingKind, BlockAssembler, ChannelDecimator, FrfPipeline, RecordingReader, RecordingWriter,
    SpectrumOutput, SpectrumPipeline, WindowKind,
};

const TAU: f64 = std::f64::consts::TAU;
const G_TO_IN_S2: f64 = 386.0886;

// ---------------------------------------------------------------------------
// Helpers (independent of the crate under test)
// ---------------------------------------------------------------------------

/// Seeded xorshift64 Gaussian noise source (Box-Muller). Deterministic.
struct NoiseGen {
    state: u64,
    spare: Option<f64>,
}

impl NoiseGen {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.max(1),
            spare: None,
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Uniform in (0, 1).
    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Standard normal via Box-Muller.
    fn gaussian(&mut self) -> f64 {
        if let Some(s) = self.spare.take() {
            return s;
        }
        let u1 = self.uniform();
        let u2 = self.uniform();
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = TAU * u2;
        self.spare = Some(r * theta.sin());
        r * theta.cos()
    }

    fn gaussian_vec(&mut self, n: usize, sigma: f64) -> Vec<f64> {
        (0..n).map(|_| sigma * self.gaussian()).collect()
    }
}

/// Independent direct-form-II-transposed biquad (RBJ low-pass resonator) with
/// a closed-form `H(e^{jw})` - the same independent-reference pattern as the
/// eigen-dsp FRF tests. NOT built on the crate's FFT.
#[derive(Clone)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn resonator(f0: f64, q: f64, fs: f64) -> Self {
        let w0 = TAU * f0 / fs;
        let (sn, cs) = (w0.sin(), w0.cos());
        let alpha = sn / (2.0 * q);
        let b0n = (1.0 - cs) / 2.0;
        let b1n = 1.0 - cs;
        let b2n = (1.0 - cs) / 2.0;
        let a0 = 1.0 + alpha;
        Self {
            b0: b0n / a0,
            b1: b1n / a0,
            b2: b2n / a0,
            a1: -2.0 * cs / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn step(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    /// Analytic `H(e^{jw})` at `f` Hz, straight from the z-domain
    /// coefficients.
    fn h_at(&self, f: f64, fs: f64) -> (f64, f64) {
        let w = TAU * f / fs;
        let (c1, s1) = ((-w).cos(), (-w).sin());
        let (c2, s2) = ((-2.0 * w).cos(), (-2.0 * w).sin());
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = self.b1 * s1 + self.b2 * s2;
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = self.a1 * s1 + self.a2 * s2;
        let den = den_re * den_re + den_im * den_im;
        (
            (num_re * den_re + num_im * den_im) / den,
            (num_im * den_re - num_re * den_im) / den,
        )
    }
}

fn cmag(c: (f64, f64)) -> f64 {
    (c.0 * c.0 + c.1 * c.1).sqrt()
}

/// Wrapped absolute phase difference in radians.
fn phase_diff(a: f64, b: f64) -> f64 {
    let mut d = (a - b).abs();
    if d > std::f64::consts::PI {
        d = TAU - d;
    }
    d
}

fn acq(
    native_rate_hz: f64,
    analysis_decimation: u32,
    block_size: usize,
    window: WindowKind,
    overlap_pct: f64,
    averaging: AveragingKind,
    average_count: usize,
) -> AcquisitionConfig {
    AcquisitionConfig {
        native_rate_hz,
        analysis_decimation,
        block_size,
        window,
        overlap_pct,
        averaging,
        average_count,
    }
}

/// Feed a single-channel signal through BlockAssembler -> SpectrumPipeline
/// (the live streaming shape) and return the pipeline + block count.
fn run_spectrum(signal: &[f64], cfg: &AcquisitionConfig) -> (SpectrumPipeline, usize) {
    let mut asm = BlockAssembler::from_config(1, cfg);
    let mut pipe = SpectrumPipeline::new(cfg);
    asm.push(0, signal);
    let mut n_blocks = 0;
    while let Some(blocks) = asm.next_block() {
        pipe.push_block(&blocks[0]);
        n_blocks += 1;
    }
    (pipe, n_blocks)
}

/// Feed an x/y pair through BlockAssembler -> FrfPipeline (lockstep).
fn run_frf(x: &[f64], y: &[f64], cfg: &AcquisitionConfig) -> FrfPipeline {
    let mut asm = BlockAssembler::from_config(2, cfg);
    let mut frf = FrfPipeline::new(cfg);
    asm.push(0, x);
    asm.push(1, y);
    while let Some(blocks) = asm.next_block() {
        frf.push_block(&blocks[0], &blocks[1]);
    }
    frf
}

/// A unique-per-test temp root (same pattern as tests/recording.rs).
fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "eigen-recorder-analysis-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn table(n: usize) -> ChannelTable {
    let mut t = ChannelTable::new();
    for i in 0..n {
        t.channels.push(Channel::new(i as u32 + 1));
    }
    t
}

// ---------------------------------------------------------------------------
// EU taxonomy: EUpk / EUrms / autopower on a pure tone
// ---------------------------------------------------------------------------

#[test]
fn eupk_eurms_autopower_on_pure_tone_with_flattop() {
    // A = 2.0 tone, deliberately OFF-bin (flattop's scalloping-free amplitude
    // readout is the point of the window).
    let fs = 8192.0;
    let a = 2.0;
    let f0 = 1003.0; // bin 125.375 at df = 8
    let cfg = acq(
        fs,
        1,
        1024,
        WindowKind::Flattop,
        75.0,
        AveragingKind::Linear,
        8,
    );
    let n = 4096;
    let signal: Vec<f64> = (0..n)
        .map(|i| a * (TAU * f0 * i as f64 / fs).sin())
        .collect();
    let (pipe, _) = run_spectrum(&signal, &cfg);
    assert!(pipe.complete(), "average should complete");
    assert_eq!(pipe.df_hz(), 8.0);
    // Flattop ENBW ~3.77 bins, derived from the actual window vector.
    assert!(
        (pipe.enbw_bins() - 3.77).abs() < 0.05,
        "flattop ENBW {}",
        pipe.enbw_bins()
    );

    let ap = pipe.autopower();
    let (peak_bin, ap_peak) =
        ap.iter().enumerate().fold(
            (0, 0.0f64),
            |(bk, bm), (k, &v)| {
                if v > bm {
                    (k, v)
                } else {
                    (bk, bm)
                }
            },
        );
    let expect_bin = (f0 / pipe.df_hz()).round() as isize;
    assert!(
        (peak_bin as isize - expect_bin).abs() <= 1,
        "tone peak bin {peak_bin} vs expected ~{expect_bin}"
    );
    // Autopower peak bin reads A^2/2 = 2.0 EU^2 rms.
    assert!(
        (ap_peak / (a * a / 2.0) - 1.0).abs() < 0.01,
        "autopower peak {ap_peak} vs {}",
        a * a / 2.0
    );
    // EUpk peak bin reads A = 2.0; EUrms reads A/sqrt(2).
    let eupk = pipe.linear_spectrum(SpectrumOutput::EuPk)[peak_bin];
    let eurms = pipe.linear_spectrum(SpectrumOutput::EuRms)[peak_bin];
    assert!((eupk / a - 1.0).abs() < 0.01, "EUpk peak {eupk} vs {a}");
    assert!(
        (eurms / (a / 2.0f64.sqrt()) - 1.0).abs() < 0.01,
        "EUrms peak {eurms} vs {}",
        a / 2.0f64.sqrt()
    );
}

#[test]
#[should_panic(expected = "linear_spectrum takes EuPk or EuRms")]
fn linear_spectrum_rejects_power_output_types() {
    let cfg = AcquisitionConfig::default();
    let pipe = SpectrumPipeline::new(&cfg);
    let _ = pipe.linear_spectrum(SpectrumOutput::Eu2PerHz);
}

// ---------------------------------------------------------------------------
// PSD level + cross-window invariance
// ---------------------------------------------------------------------------

#[test]
fn psd_level_on_seeded_noise_and_cross_window_invariance() {
    // White noise of known variance: one-sided PSD = 2*sigma^2/fs. The PSD
    // band level must be WINDOW-INDEPENDENT (energy correction domain) -
    // Hann vs Flattop vs Rectangular agree within 0.5 dB.
    let fs = 8192.0;
    let sigma = 1.5;
    let expected = 2.0 * sigma * sigma / fs;
    let signal = NoiseGen::new(0xD5EED01).gaussian_vec(50_000, sigma);

    let mut band_means = Vec::new();
    for window in [
        WindowKind::Hann,
        WindowKind::Flattop,
        WindowKind::Rectangular,
    ] {
        let cfg = acq(fs, 1, 1024, window, 50.0, AveragingKind::Linear, 64);
        let (pipe, _) = run_spectrum(&signal, &cfg);
        assert!(pipe.complete());
        let psd = pipe.psd();
        let band = &psd[16..400];
        let mean = band.iter().sum::<f64>() / band.len() as f64;
        assert!(
            (mean / expected - 1.0).abs() < 0.05,
            "{window:?} PSD band mean {mean} vs expected {expected}"
        );
        band_means.push(mean);
    }
    for i in 0..band_means.len() {
        for j in (i + 1)..band_means.len() {
            let db = 10.0 * (band_means[i] / band_means[j]).log10();
            assert!(
                db.abs() < 0.5,
                "cross-window PSD mismatch {db} dB ({i} vs {j})"
            );
        }
    }
}

#[test]
fn parseval_psd_integral_matches_signal_power() {
    // Exact single-block case: rectangular window, one block -> the PSD
    // integral equals the block's mean-removed power (the periodogram removes
    // the per-block mean) to float rounding.
    let fs = 4096.0;
    let cfg = acq(
        fs,
        1,
        1024,
        WindowKind::Rectangular,
        0.0,
        AveragingKind::Linear,
        1,
    );
    let block = NoiseGen::new(0xA55E7A1).gaussian_vec(1024, 1.0);
    let (pipe, n_blocks) = run_spectrum(&block, &cfg);
    assert_eq!(n_blocks, 1);
    let psd = pipe.psd();
    let integral: f64 = psd.iter().sum::<f64>() * pipe.df_hz();
    let mean = block.iter().sum::<f64>() / block.len() as f64;
    let var = block.iter().map(|&x| (x - mean) * (x - mean)).sum::<f64>() / block.len() as f64;
    assert!(
        (integral / var - 1.0).abs() < 1e-9,
        "Parseval: integral {integral} vs variance {var}"
    );

    // Statistical multi-block case with Hann: integral ~ sigma^2 (time RMS^2
    // of the zero-mean noise) within tolerance.
    let sigma = 1.25;
    let signal = NoiseGen::new(0xBEEF02).gaussian_vec(40_000, sigma);
    let cfg2 = acq(
        fs,
        1,
        1024,
        WindowKind::Hann,
        50.0,
        AveragingKind::Linear,
        32,
    );
    let (pipe2, _) = run_spectrum(&signal, &cfg2);
    let integral2: f64 = pipe2.psd().iter().sum::<f64>() * pipe2.df_hz();
    assert!(
        (integral2 / (sigma * sigma) - 1.0).abs() < 0.05,
        "Parseval (Hann, averaged): integral {integral2} vs sigma^2 {}",
        sigma * sigma
    );
}

#[test]
fn autopower_equals_psd_times_df_times_enbw() {
    // The identity tying the amplitude and energy correction domains together:
    // autopower[k] == psd[k] * df * enbw_bins, elementwise.
    let fs = 8192.0;
    let cfg = acq(
        fs,
        1,
        2048,
        WindowKind::Hann,
        50.0,
        AveragingKind::Linear,
        16,
    );
    let signal = NoiseGen::new(0x1DE17).gaussian_vec(40_000, 0.8);
    let (pipe, _) = run_spectrum(&signal, &cfg);
    let psd = pipe.psd();
    let ap = pipe.autopower();
    let scale = pipe.df_hz() * pipe.enbw_bins();
    let floor = psd.iter().fold(0.0f64, |m, &v| m.max(v)) * 1e-12;
    let mut checked = 0;
    for (k, (&p, &a)) in psd.iter().zip(ap.iter()).enumerate() {
        if p > floor {
            let rel = (a - p * scale).abs() / (p * scale);
            assert!(rel < 1e-9, "identity failed at bin {k}: rel {rel}");
            checked += 1;
        }
    }
    assert!(checked > 900, "too few bins checked ({checked})");
}

// ---------------------------------------------------------------------------
// FRF: H1 golden against the analytic biquad, Hv identity, coherence trap
// ---------------------------------------------------------------------------

#[test]
fn h1_recovers_analytic_biquad_with_unity_coherence() {
    // Clean case: y = plant(x), no extraneous noise. H1 must match the
    // closed-form biquad response in magnitude AND phase (the pinned
    // Gxy = <X*conj(Y)> convention: lagging response = negative phase), and
    // coherence must be ~1 in-band.
    let fs = 8192.0;
    let cfg = acq(
        fs,
        1,
        1024,
        WindowKind::Hann,
        50.0,
        AveragingKind::Linear,
        8,
    );
    let plant = Biquad::resonator(600.0, 4.0, fs);
    let x = NoiseGen::new(0xF00D01).gaussian_vec(200_000, 1.0);
    let mut p = plant.clone();
    let y: Vec<f64> = x.iter().map(|&v| p.step(v)).collect();

    let frf = run_frf(&x, &y, &cfg);
    assert!(frf.coherence_valid());
    let h1 = frf.h1();
    let coh = frf.coherence();
    let df = frf.df_hz();

    let mut checked = 0;
    let mut worst_mag = 0.0f64;
    let mut worst_phase = 0.0f64;
    let mut worst_coh = 1.0f64;
    for (k, &he) in h1.iter().enumerate() {
        let f = k as f64 * df;
        if !(100.0..=1400.0).contains(&f) {
            continue;
        }
        let ha = plant.h_at(f, fs);
        if cmag(ha) < 0.05 {
            continue;
        }
        worst_mag = worst_mag.max((cmag(he) - cmag(ha)).abs() / cmag(ha));
        worst_phase = worst_phase.max(phase_diff(he.1.atan2(he.0), ha.1.atan2(ha.0)));
        worst_coh = worst_coh.min(coh[k]);
        checked += 1;
    }
    assert!(checked > 50, "too few bins checked ({checked})");
    assert!(worst_mag < 0.10, "worst in-band |H1| rel error {worst_mag}");
    // 2 degrees = 0.0349 rad.
    assert!(
        worst_phase < 2.0f64.to_radians(),
        "worst in-band phase error {} deg",
        worst_phase.to_degrees()
    );
    assert!(worst_coh > 0.98, "worst in-band coherence {worst_coh}");
}

#[test]
fn h1_vs_hv_output_noise_bias_and_gamma_identity() {
    // Independent output noise added to y: H1 stays (nearly) unbiased while
    // |Hv| = sqrt(Gyy/Gxx) is biased HIGH. The scalar identity
    // |H1| = gamma * |Hv| (gamma = sqrt(coherence)) is algebraic and must
    // hold to float rounding.
    let fs = 8192.0;
    let cfg = acq(
        fs,
        1,
        1024,
        WindowKind::Hann,
        50.0,
        AveragingKind::Linear,
        8,
    );
    let plant = Biquad::resonator(600.0, 4.0, fs);
    let x = NoiseGen::new(0xF00D02).gaussian_vec(200_000, 1.0);
    let noise = NoiseGen::new(0x015377).gaussian_vec(200_000, 0.3);
    let mut p = plant.clone();
    let y: Vec<f64> = x.iter().zip(&noise).map(|(&v, &n)| p.step(v) + n).collect();

    let frf = run_frf(&x, &y, &cfg);
    assert!(frf.coherence_valid());
    let h1 = frf.h1();
    let hv = frf.hv();
    let coh = frf.coherence();
    let df = frf.df_hz();

    // Identity |H1| = gamma * |Hv| at every energetic bin.
    let mut identity_checked = 0;
    for k in 0..h1.len() {
        let m1 = cmag(h1[k]);
        let mv = cmag(hv[k]);
        if mv <= 0.0 || mv.is_nan() {
            continue;
        }
        let rhs = coh[k].sqrt() * mv;
        assert!(
            (m1 - rhs).abs() <= 1e-6 * rhs.max(1e-30),
            "identity |H1| = gamma*|Hv| failed at bin {k}: {m1} vs {rhs}"
        );
        identity_checked += 1;
    }
    assert!(identity_checked > 400, "too few identity bins");

    // Bias direction: in-band, coherence is strictly < 1 and |H1| < |Hv|.
    let mut bias_checked = 0;
    let mut coh_sum = 0.0;
    for k in 0..h1.len() {
        let f = k as f64 * df;
        if !(100.0..=1400.0).contains(&f) {
            continue;
        }
        assert!(coh[k] < 0.9999, "in-band coherence {} at bin {k}", coh[k]);
        assert!(
            cmag(h1[k]) < cmag(hv[k]),
            "|H1| not below |Hv| at bin {k} with output noise"
        );
        coh_sum += coh[k];
        bias_checked += 1;
    }
    assert!(bias_checked > 50);
    let coh_mean = coh_sum / bias_checked as f64;
    assert!(
        coh_mean < 0.999 && coh_mean > 0.1,
        "in-band mean coherence {coh_mean} not in the noisy-but-correlated range"
    );
}

#[test]
fn coherence_single_average_trap_and_gate() {
    // The single-average trap AS a test: after ONE average the coherence is 1.0 identically
    // (|Gxy|^2 == Gxx*Gyy for a single block) and coherence_valid() must be
    // false so the caller suppresses the display. A second average makes the
    // gate open and the estimate informative.
    let fs = 8192.0;
    let cfg = acq(
        fs,
        1,
        1024,
        WindowKind::Hann,
        50.0,
        AveragingKind::Linear,
        8,
    );
    let x = NoiseGen::new(0x712401).gaussian_vec(1024, 1.0);
    let mut plant = Biquad::resonator(500.0, 5.0, fs);
    let noise = NoiseGen::new(0x712402).gaussian_vec(1024, 0.5);
    let y: Vec<f64> = x
        .iter()
        .zip(&noise)
        .map(|(&v, &n)| plant.step(v) + n)
        .collect();

    let mut frf = FrfPipeline::new(&cfg);
    frf.push_block(&x, &y);
    assert_eq!(frf.n_averages(), 1);
    assert!(
        !frf.coherence_valid(),
        "one average must NOT validate coherence"
    );
    for (k, &c) in frf.coherence().iter().enumerate() {
        // Bins are either dead (no energy -> 0) or numerically 1.0.
        assert!(
            c == 0.0 || (c - 1.0).abs() < 1e-9,
            "single-average coherence at bin {k} is {c}, not the degenerate 1.0"
        );
    }
    // The trap is real: despite HEAVY uncorrelated noise, one average showed
    // coherence 1. A second average opens the gate and drops coherence.
    let x2 = NoiseGen::new(0x712403).gaussian_vec(1024, 1.0);
    let noise2 = NoiseGen::new(0x712404).gaussian_vec(1024, 0.5);
    let y2: Vec<f64> = x2
        .iter()
        .zip(&noise2)
        .map(|(&v, &n)| plant.step(v) + n)
        .collect();
    frf.push_block(&x2, &y2);
    assert!(frf.coherence_valid());
    let coh = frf.coherence();
    let interior = &coh[10..500];
    let mean = interior.iter().sum::<f64>() / interior.len() as f64;
    assert!(
        mean < 0.9999,
        "two-average coherence should drop below the degenerate 1.0 (mean {mean})"
    );
}

#[test]
fn transmissibility_equals_h1_magnitude_exactly() {
    let fs = 8192.0;
    let cfg = acq(fs, 1, 512, WindowKind::Hann, 50.0, AveragingKind::Linear, 8);
    let x = NoiseGen::new(0x7A4501).gaussian_vec(20_000, 1.0);
    let mut plant = Biquad::resonator(700.0, 6.0, fs);
    let y: Vec<f64> = x.iter().map(|&v| plant.step(v)).collect();
    let frf = run_frf(&x, &y, &cfg);
    let h1 = frf.h1();
    let trans = frf.transmissibility();
    assert_eq!(trans.len(), h1.len());
    for (k, (&t, &h)) in trans.iter().zip(h1.iter()).enumerate() {
        assert_eq!(t, cmag(h), "transmissibility != |H1| at bin {k}");
    }
}

// ---------------------------------------------------------------------------
// Time statistics
// ---------------------------------------------------------------------------

#[test]
fn time_stats_sine_crest_and_kurtosis_goldens() {
    // 64 whole cycles, 128 samples per cycle: RMS = A/sqrt(2) exactly, the
    // peak sample lands exactly on A, sine kurtosis = 1.5 analytic.
    let a = 1.5;
    let n = 8192;
    let x: Vec<f64> = (0..n).map(|i| a * (TAU * i as f64 / 128.0).sin()).collect();
    let s = time_stats(&x);
    assert!(
        (s.crest - 2.0f64.sqrt()).abs() / 2.0f64.sqrt() < 1e-3,
        "sine crest {} vs sqrt(2)",
        s.crest
    );
    assert!(
        (s.kurtosis - 1.5).abs() < 1e-6,
        "sine kurtosis {} vs 1.5 (vibration convention)",
        s.kurtosis
    );
    assert!(s.skew.abs() < 1e-6, "sine skew {}", s.skew);
    assert!(s.mean.abs() < 1e-12, "sine mean {}", s.mean);
    assert!(
        (s.rms - a / 2.0f64.sqrt()).abs() < 1e-9,
        "sine rms {}",
        s.rms
    );
    assert!((s.peak - a).abs() < 1e-12, "sine peak {}", s.peak);
    assert!((s.pk_pk - 2.0 * a).abs() < 1e-12, "sine pk-pk {}", s.pk_pk);
}

#[test]
fn time_stats_seeded_gaussian_kurtosis_near_three() {
    // Vibration-convention kurtosis of a Gaussian is 3. Sampling
    // std-err of the kurtosis estimate is ~sqrt(24/N); allow 3 sigma.
    let n = 131_072; // >= 65,536 per the gate
    let sigma = 2.0;
    let x = NoiseGen::new(0x6A055E).gaussian_vec(n, sigma);
    let s = time_stats(&x);
    let tol_k = 3.0 * (24.0 / n as f64).sqrt();
    assert!(
        (s.kurtosis - 3.0).abs() < tol_k,
        "Gaussian kurtosis {} vs 3 +/- {tol_k}",
        s.kurtosis
    );
    let tol_s = 3.0 * (6.0 / n as f64).sqrt();
    assert!(s.skew.abs() < tol_s, "Gaussian skew {}", s.skew);
    assert!((s.rms / sigma - 1.0).abs() < 0.02, "Gaussian rms {}", s.rms);

    // Convention cross-check: mean/rms/peak match eigen_dsp::stats exactly
    // (the streaming accumulator implements the same definitions).
    assert!((s.mean - eigen_dsp::stats::mean(&x)).abs() < 1e-12);
    assert!((s.rms - eigen_dsp::stats::rms(&x)).abs() < 1e-12);
    assert_eq!(s.peak, eigen_dsp::stats::peak(&x));

    // Empty input: all zeros, no NaN.
    let e = time_stats(&[]);
    assert_eq!(e.rms, 0.0);
    assert_eq!(e.crest, 0.0);
    assert_eq!(e.kurtosis, 0.0);
}

// ---------------------------------------------------------------------------
// Decimated end-to-end pipeline
// ---------------------------------------------------------------------------

#[test]
fn decimated_pipeline_tone_lands_on_frequency_and_amplitude() {
    // 100 Hz tone at native 65,536 Hz, analysis decimation 16 -> 4,096 Hz
    // analysis rate, block 2048 -> df 2 Hz. The spectrum peak must land at
    // 100 Hz within one bin and read the amplitude within 0.2 dB (exercises
    // the DecimatorCascade end-to-end: anti-alias passband flatness + the
    // pipeline normalizations).
    let fs = 65_536.0;
    let a = 1.5;
    let f0 = 100.0;
    let cfg = acq(
        fs,
        16,
        2048,
        WindowKind::Flattop,
        75.0,
        AveragingKind::Linear,
        8,
    );
    assert_eq!(cfg.analysis_rate_hz(), 4096.0);
    let n = 262_144; // 4 s
    let signal: Vec<f64> = (0..n)
        .map(|i| a * (TAU * f0 * i as f64 / fs).sin())
        .collect();

    let mut decim = ChannelDecimator::new(1, &cfg);
    assert_eq!(decim.factor(), 16);
    let mut asm = BlockAssembler::from_config(1, &cfg);
    let mut pipe = SpectrumPipeline::new(&cfg);
    // Stream in uneven chunks to exercise the stateful cascade.
    for chunk in signal.chunks(10_000) {
        let dec = decim.process(0, chunk);
        asm.push(0, &dec);
        while let Some(blocks) = asm.next_block() {
            pipe.push_block(&blocks[0]);
        }
    }
    assert!(pipe.complete());
    assert_eq!(pipe.df_hz(), 2.0);

    let eupk = pipe.linear_spectrum(SpectrumOutput::EuPk);
    let (peak_bin, peak) =
        eupk.iter().enumerate().fold(
            (0, 0.0f64),
            |(bk, bm), (k, &v)| {
                if v > bm {
                    (k, v)
                } else {
                    (bk, bm)
                }
            },
        );
    let expect_bin = (f0 / pipe.df_hz()).round() as isize;
    assert!(
        (peak_bin as isize - expect_bin).abs() <= 1,
        "decimated tone peak bin {peak_bin} vs expected {expect_bin}"
    );
    let db = 20.0 * (peak / a).log10();
    assert!(
        db.abs() < 0.2,
        "decimated tone amplitude error {db} dB (peak {peak} vs {a})"
    );
}

// ---------------------------------------------------------------------------
// BlockAssembler mechanics
// ---------------------------------------------------------------------------

#[test]
fn block_assembler_hop_math_and_lockstep() {
    // hop = round(block * (1 - ov/100)), floored at 1.
    assert_eq!(hop_frames(1024, 50.0), 512);
    assert_eq!(hop_frames(1024, 75.0), 256);
    assert_eq!(hop_frames(1024, 0.0), 1024);
    assert_eq!(hop_frames(4, 95.0), 1);
    assert_eq!(hop_frames(1024, 66.7), 341);

    let mut asm = BlockAssembler::new(2, 8, 50.0);
    assert_eq!(asm.hop(), 4);
    let ch0: Vec<f64> = (0..12).map(f64::from).collect();
    let ch1: Vec<f64> = (100..108).map(f64::from).collect();
    asm.push(0, &ch0);
    // LOCKSTEP: channel 1 has nothing yet -> no block, even though channel 0
    // has more than a full block.
    assert!(asm.next_block().is_none());
    asm.push(1, &ch1);
    let b = asm.next_block().expect("both channels full");
    assert_eq!(b[0], (0..8).map(f64::from).collect::<Vec<_>>());
    assert_eq!(b[1], (100..108).map(f64::from).collect::<Vec<_>>());
    // Advanced by hop 4: channel 0 has 12 - 4 = 8 available, channel 1 only 4.
    assert!(asm.next_block().is_none());
    asm.push(1, &[108.0, 109.0, 110.0, 111.0]);
    let b2 = asm.next_block().expect("second lockstep block");
    assert_eq!(b2[0], (4..12).map(f64::from).collect::<Vec<_>>());
    assert_eq!(b2[1], (104..112).map(f64::from).collect::<Vec<_>>());
    assert!(asm.next_block().is_none());

    // Decimation factor 1 is a bit-exact pass-through.
    let cfg = AcquisitionConfig::default();
    let mut decim = ChannelDecimator::new(1, &cfg);
    assert_eq!(decim.factor(), 1);
    let out = decim.process(0, &ch0);
    assert_eq!(out, ch0);
}

// ---------------------------------------------------------------------------
// Spectral omega-arithmetic
// ---------------------------------------------------------------------------

#[test]
fn psd_omega_arithmetic_units_and_dc_zero() {
    // Flat 1.0 g^2/Hz accel density: vel[k] = G^2/(2 pi k df)^2 (in/s)^2/Hz,
    // disp[k] = vel[k]/(2 pi k df)^2 in^2/Hz, DC -> 0 at every stage.
    let df = 2.0;
    let accel = vec![1.0f64; 65];
    let vel = psd_accel_to_velocity(&accel, df);
    let disp_chain = psd_accel_to_displacement(&accel, df);
    let disp_two_step = psd_velocity_to_displacement(&vel, df);
    assert_eq!(vel[0], 0.0, "velocity DC bin");
    assert_eq!(disp_chain[0], 0.0, "displacement DC bin");
    for k in 1..accel.len() {
        let w = TAU * k as f64 * df;
        let expect_v = G_TO_IN_S2 * G_TO_IN_S2 / (w * w);
        assert!(
            (vel[k] - expect_v).abs() < 1e-9 * expect_v,
            "vel psd at bin {k}: {} vs {expect_v}",
            vel[k]
        );
        let expect_d = expect_v / (w * w);
        assert!(
            (disp_chain[k] - expect_d).abs() < 1e-9 * expect_d,
            "disp psd at bin {k}"
        );
        assert_eq!(disp_chain[k], disp_two_step[k], "chain != two-step at {k}");
    }
}

#[test]
fn integrate_region_trims_settle_transient() {
    // Step-in 50 Hz sine (starts abruptly at the region start). The trim
    // count must match ~5/(2 pi hp_hz) seconds, and the post-trim velocity
    // must be settled: the two halves of the trimmed window have the same RMS
    // and match the analytic sine-integral amplitude.
    let fs = 4096.0;
    let hp = 5.0;
    let f0 = 50.0;
    let n = 16_384;
    let accel: Vec<f64> = (0..n).map(|i| (TAU * f0 * i as f64 / fs).sin()).collect();
    let (vel, disp, trim) = integrate_region(&accel, fs, hp);

    let expected_trim = (5.0 / (TAU * hp) * fs).ceil() as usize;
    assert_eq!(trim, expected_trim, "trim count vs the ~5 tau formula");
    assert_eq!(vel.len(), n - trim);
    assert_eq!(disp.len(), n - trim);

    let half = vel.len() / 2;
    let rms = |xs: &[f64]| (xs.iter().map(|v| v * v).sum::<f64>() / xs.len() as f64).sqrt();
    let r1 = rms(&vel[..half]);
    let r2 = rms(&vel[half..]);
    assert!(
        (r1 / r2 - 1.0).abs() < 0.02,
        "post-trim velocity RMS not stable: {r1} vs {r2}"
    );
    // Analytic settled amplitude: G*A/w, times the first-order HP passband
    // gain at 50 Hz with fc = 5 Hz -> (f/fc)/sqrt(1+(f/fc)^2).
    let w = TAU * f0;
    let hp_gain = (f0 / hp) / (1.0 + (f0 / hp) * (f0 / hp)).sqrt();
    let expected_rms = G_TO_IN_S2 / w * hp_gain / 2.0f64.sqrt();
    let r_all = rms(&vel);
    assert!(
        (r_all / expected_rms - 1.0).abs() < 0.03,
        "post-trim velocity RMS {r_all} vs analytic {expected_rms}"
    );

    // hp disabled -> nothing to settle, nothing trimmed.
    let (v0, d0, t0) = integrate_region(&accel, fs, 0.0);
    assert_eq!(t0, 0);
    assert_eq!(v0.len(), n);
    assert_eq!(d0.len(), n);
}

// ---------------------------------------------------------------------------
// Live-vs-offline equivalence through a real recording
// ---------------------------------------------------------------------------

/// Interleave two channels into f32 frames (the recorder's storage type).
fn interleave2_f32(ch0: &[f64], ch1: &[f64]) -> Vec<f32> {
    ch0.iter()
        .zip(ch1)
        .flat_map(|(&a, &b)| [a as f32, b as f32])
        .collect()
}

/// The f64 samples of one channel as the analysis path sees them
/// (f32-quantized by storage, widened back to f64).
fn channel_f64(frames: &[f32], n_ch: usize, ch: usize) -> Vec<f64> {
    frames
        .chunks_exact(n_ch)
        .map(|fr| f64::from(fr[ch]))
        .collect()
}

fn rel_close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * a.abs().max(b.abs()).max(1e-300)
}

#[test]
fn analyze_region_matches_live_pipeline_bit_for_bit() {
    // Write a 2-channel recording, then: (offline) analyze_region through the
    // chunked reader, (live) feed the identical f32-quantized samples through
    // the same ChannelDecimator -> BlockAssembler -> SpectrumPipeline structs
    // directly. Results must agree to 1e-12 relative (one math).
    let root = temp_root("live-vs-offline");
    let fs = 8192.0;
    let n = 16_384usize;
    let cfg = acq(fs, 2, 512, WindowKind::Hann, 50.0, AveragingKind::Linear, 8);

    let ch0 = NoiseGen::new(0x11FE01).gaussian_vec(n, 1.0);
    let ch1: Vec<f64> = ch0
        .iter()
        .enumerate()
        .map(|(i, &v)| 0.5 * v + (TAU * 200.0 * i as f64 / fs).sin())
        .collect();
    let frames = interleave2_f32(&ch0, &ch1);

    let mut writer = RecordingWriter::start(&root, "region", &cfg, table(2), None).unwrap();
    writer.push_frames(&frames, n);
    let summary = writer.finish().unwrap();
    assert!(summary.gap_free);
    assert_eq!(summary.frames_written, n as u64);

    let mut reader = RecordingReader::open(&summary.run_dir).unwrap();
    let offline = analyze_region(&mut reader, &[0, 1], 0, n, &cfg).unwrap();
    assert_eq!(offline.per_channel.len(), 2);

    // Live path over the SAME f32-quantized samples.
    for (idx, ch) in [0usize, 1].into_iter().enumerate() {
        let samples = channel_f64(&frames, 2, ch);
        let mut decim = ChannelDecimator::new(1, &cfg);
        let mut asm = BlockAssembler::from_config(1, &cfg);
        let mut pipe = SpectrumPipeline::new(&cfg);
        let mut live_blocks = 0;
        let dec = decim.process(0, &samples);
        asm.push(0, &dec);
        while let Some(blocks) = asm.next_block() {
            pipe.push_block(&blocks[0]);
            live_blocks += 1;
        }
        let off = &offline.per_channel[idx];
        assert_eq!(off.channel, ch);
        assert_eq!(offline.n_blocks, live_blocks);
        assert_eq!(off.df_hz, pipe.df_hz());

        let live_psd = pipe.psd();
        let live_ap = pipe.autopower();
        let live_lin = pipe.linear_spectrum(SpectrumOutput::EuRms);
        assert_eq!(off.psd.len(), live_psd.len());
        for k in 0..live_psd.len() {
            assert!(
                rel_close(off.psd[k], live_psd[k], 1e-12),
                "psd mismatch ch{ch} bin {k}: {} vs {}",
                off.psd[k],
                live_psd[k]
            );
            assert!(
                rel_close(off.autopower[k], live_ap[k], 1e-12),
                "autopower mismatch ch{ch} bin {k}"
            );
            assert!(
                rel_close(off.linear_rms[k], live_lin[k], 1e-12),
                "linear_rms mismatch ch{ch} bin {k}"
            );
        }
        // Stats over the raw native-rate samples: same accumulator math.
        let live_stats = time_stats(&samples);
        assert!(rel_close(off.stats.rms, live_stats.rms, 1e-12));
        assert!(rel_close(off.stats.kurtosis, live_stats.kurtosis, 1e-12));
        assert_eq!(off.stats.peak, live_stats.peak);
        assert_eq!(off.stats.pk_pk, live_stats.pk_pk);
    }
}

#[test]
fn analyze_region_frf_matches_live_pipeline() {
    let root = temp_root("frf-offline");
    let fs = 8192.0;
    let n = 32_768usize;
    let cfg = acq(fs, 2, 512, WindowKind::Hann, 50.0, AveragingKind::Linear, 8);

    let x = NoiseGen::new(0xFEF01).gaussian_vec(n, 1.0);
    let mut plant = Biquad::resonator(300.0, 5.0, fs);
    let y: Vec<f64> = x.iter().map(|&v| plant.step(v)).collect();
    let frames = interleave2_f32(&x, &y);

    let mut writer = RecordingWriter::start(&root, "frfrun", &cfg, table(2), None).unwrap();
    writer.push_frames(&frames, n);
    let summary = writer.finish().unwrap();
    let mut reader = RecordingReader::open(&summary.run_dir).unwrap();
    let offline = analyze_region_frf(&mut reader, 0, 1, 0, n, &cfg).unwrap();

    // Live path over the same f32-quantized pair.
    let xs = channel_f64(&frames, 2, 0);
    let ys = channel_f64(&frames, 2, 1);
    let mut decim = ChannelDecimator::new(2, &cfg);
    let mut asm = BlockAssembler::from_config(2, &cfg);
    let mut live = FrfPipeline::new(&cfg);
    let dx = decim.process(0, &xs);
    let dy = decim.process(1, &ys);
    asm.push(0, &dx);
    asm.push(1, &dy);
    while let Some(blocks) = asm.next_block() {
        live.push_block(&blocks[0], &blocks[1]);
    }

    assert_eq!(offline.n_averages, live.n_averages());
    assert!(offline.coherence_valid);
    assert_eq!(offline.df_hz, live.df_hz());
    let (lh1, lhv, lcoh, ltr) = (
        live.h1(),
        live.hv(),
        live.coherence(),
        live.transmissibility(),
    );
    for k in 0..lh1.len() {
        assert!(
            rel_close(offline.h1[k].0, lh1[k].0, 1e-12)
                && rel_close(offline.h1[k].1, lh1[k].1, 1e-12),
            "h1 mismatch at bin {k}"
        );
        assert!(
            rel_close(offline.hv[k].0, lhv[k].0, 1e-12)
                && rel_close(offline.hv[k].1, lhv[k].1, 1e-12),
            "hv mismatch at bin {k}"
        );
        assert!(
            rel_close(offline.coherence[k], lcoh[k], 1e-12),
            "coherence mismatch at bin {k}"
        );
        assert!(
            rel_close(offline.transmissibility[k], ltr[k], 1e-12),
            "transmissibility mismatch at bin {k}"
        );
    }

    // Out-of-range channel is a clean error, not a panic.
    assert!(analyze_region_frf(&mut reader, 0, 7, 0, n, &cfg).is_err());
}
