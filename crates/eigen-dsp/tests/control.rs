//! Closed-loop control DSP tests for `eigen-dsp`:
//! `cola::ColaSynth`, `frf::h_estimate_hv`, `frf::h_inverse_regularized`,
//! `sine::TrackingFilter`, `sine::AmplitudeServo`.
//!
//! Every reference is analytic and independent of the implementation's own
//! intermediate results: the FRF tests compare against a closed-form biquad
//! transfer function evaluated by hand (not via the crate's FFT), the servo tests
//! run a plant model defined in-test, and the tracking-filter test compares against
//! the exact amplitude/phase of a synthetic tone.

use eigen_dsp::{cola, frf, sine, spectrum, stats};

// ---------------------------------------------------------------------------
// Small independent helpers (NOT reusing the crate's control internals).
// ---------------------------------------------------------------------------

/// A direct-form-II transposed biquad `H(z) = (b0 + b1 z^-1 + b2 z^-2) /
/// (1 + a1 z^-1 + a2 z^-2)`, used as an independent synthetic plant.
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
    /// RBJ resonator (constant-skirt-gain bandpass-ish 2nd order low-pass with a
    /// Q peak) at natural frequency `f0` and quality `q`, sample rate `fs`.
    /// Low-pass prototype so it has a well-defined DC gain and a resonance peak.
    fn resonator(f0: f64, q: f64, fs: f64) -> Self {
        let w0 = std::f64::consts::TAU * f0 / fs;
        let (sn, cs) = (w0.sin(), w0.cos());
        let alpha = sn / (2.0 * q);
        // RBJ low-pass.
        let b0n = (1.0 - cs) / 2.0;
        let b1n = 1.0 - cs;
        let b2n = (1.0 - cs) / 2.0;
        let a0 = 1.0 + alpha;
        let a1n = -2.0 * cs;
        let a2n = 1.0 - alpha;
        Self {
            b0: b0n / a0,
            b1: b1n / a0,
            b2: b2n / a0,
            a1: a1n / a0,
            a2: a2n / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn step(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    /// Analytic frequency response `H(e^{j w})` at frequency `f` (Hz), computed
    /// directly from the z-domain coefficients. Independent of the crate FFT.
    fn h_at(&self, f: f64, fs: f64) -> (f64, f64) {
        let w = std::f64::consts::TAU * f / fs;
        // e^{-jw}, e^{-2jw}
        let (c1, s1) = ((-w).cos(), (-w).sin());
        let (c2, s2) = ((-2.0 * w).cos(), (-2.0 * w).sin());
        // numerator b0 + b1 e^-jw + b2 e^-2jw
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = self.b1 * s1 + self.b2 * s2;
        // denominator 1 + a1 e^-jw + a2 e^-2jw
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

/// Complex multiply.
fn cmul(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

// ---------------------------------------------------------------------------
// COLA
// ---------------------------------------------------------------------------

#[test]
fn cola_condition_holds_for_sqrt_hann_50_and_75() {
    // For random synthesis the SQUARED-window overlap sum W2 sets the (flat)
    // variance and must be constant. With sqrt-Hann at 50% overlap W2 == 1.0
    // exactly; construction panics if the squared-window COLA condition fails.
    let s50 = cola::ColaSynth::new(256, 0.5, 4096.0);
    assert!(
        (s50.squared_window_cola() - 1.0).abs() < 1e-9,
        "sqrt-Hann-50% W2 {}",
        s50.squared_window_cola()
    );
    let s75 = cola::ColaSynth::new(256, 0.75, 4096.0);
    assert!(s75.squared_window_cola() > 0.0);
    assert_eq!(s50.hop(), 128);
    assert_eq!(s75.hop(), 64);
}

#[test]
#[should_panic(expected = "COLA")]
fn cola_rejects_non_cola_overlap() {
    // 1/3 overlap (hop = 2N/3) is not a Hann-COLA point; construction must reject.
    let _ = cola::ColaSynth::new(256, 1.0 / 3.0, 4096.0);
}

#[test]
fn cola_overlap_adds_without_amplitude_ripple() {
    // A gap-free, stationary stream must have no periodic amplitude modulation at
    // the hop period. Measure the local RMS in consecutive hop-sized windows; its
    // coefficient of variation must be small (only random fluctuation, no
    // deterministic scalloping locked to the frame boundary).
    let fs = 4096.0;
    let nfft = 256;
    let mut synth = cola::ColaSynth::with_seed(nfft, 0.5, fs, 0xABCD);
    let target = vec![1.0f64; nfft / 2 + 1];
    let hop = synth.hop();

    // Many frames so the per-position variance estimate has low sampling noise;
    // any residual frame-locked scalloping would then stand out clearly.
    let nframes = 40_000usize;
    let mut stream = Vec::new();
    for _ in 0..nframes {
        stream.extend_from_slice(&synth.next_frame(&target));
    }
    let stream = &stream[nfft..]; // drop startup transient

    // Local RMS per hop window.
    let nwin = stream.len() / hop;
    let mut local: Vec<f64> = Vec::with_capacity(nwin);
    for w in 0..nwin {
        local.push(stats::rms(&stream[w * hop..(w + 1) * hop]));
    }
    let mean = stats::mean(&local);
    let sd = (local.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / local.len() as f64).sqrt();
    let cov = sd / mean;
    // For an independent-per-hop random signal the RMS estimate over `hop` samples
    // has a CoV ~ 1/sqrt(2*hop) ~ 0.06 for hop=128. Deterministic scalloping would
    // add a coherent component and push CoV well past that.
    assert!(cov < 0.09, "excess amplitude ripple: CoV = {cov}");

    // KEY anti-scalloping check: the mean-square of samples at each within-hop
    // POSITION, averaged over all frames, must be flat. A plain-Hann synthesis
    // window would show W2 = 0.5 + 0.5*cos^2 here -> ~30% peak-to-mean structure.
    // sqrt-Hann makes W2 flat, so the residual is only sampling noise.
    let mut phase_var = vec![0.0f64; hop];
    for w in 0..nwin {
        for (p, pv) in phase_var.iter_mut().enumerate() {
            *pv += stream[w * hop + p].powi(2);
        }
    }
    for pv in phase_var.iter_mut() {
        *pv /= nwin as f64; // per-position mean square (variance)
    }
    let vmean = stats::mean(&phase_var);
    let vdev = phase_var
        .iter()
        .map(|v| (v - vmean).abs())
        .fold(0.0, f64::max)
        / vmean;
    // With ~80k windows the per-position variance sampling error is ~sqrt(2/nwin)
    // ~ 0.5%, so a flat W2 keeps the max deviation small. Well under the ~30% a
    // plain-Hann window would produce.
    assert!(
        vdev < 0.06,
        "amplitude scalloping vs frame position: {vdev}"
    );
}

#[test]
fn cola_stream_variance_matches_flat_target() {
    // GROUND TRUTH for synthesis power: the time-domain variance of the stream
    // equals the integral of the target one-sided PSD, S0 * fs/2, independent of
    // any Welch re-analysis bias. This is the tight (3%) power check.
    let fs = 4096.0;
    let nfft = 512;
    let s0 = 2.5; // flat EU^2/Hz
    let mut synth = cola::ColaSynth::with_seed(nfft, 0.5, fs, 0x1234_5678);
    let target = vec![s0; nfft / 2 + 1];

    let mut stream = Vec::new();
    for _ in 0..8000 {
        stream.extend_from_slice(&synth.next_frame(&target));
    }
    let stream = &stream[nfft..];
    let var = stats::rms(stream).powi(2);
    let expected = s0 * fs / 2.0;
    let ratio = var / expected;
    assert!(
        (ratio - 1.0).abs() < 0.03,
        "stream variance ratio {ratio} (var {var}, expected {expected})"
    );
}

#[test]
fn cola_welch_psd_matches_flat_target_within_tolerance() {
    // Re-analyze the synthesized stream with the crate Welch estimator and confirm
    // the recovered INTERIOR PSD matches the flat target. The re-analysis error at
    // matched 50% overlap is small: numpy + crate cross-checks measure ~0.2-0.6%
    // interior error (there is NO ~5.6% "analysis-window re-correlation" bias; that
    // earlier rationale was wrong and masked the real band-edge defects). The
    // tolerance below is set to the measured interior error plus sampling margin,
    // NOT loosened to hide the edge bins - those are checked separately in
    // `cola_edge_bins_are_documented_limitation`.
    let fs = 4096.0;
    let nfft = 256;
    let s0 = 4.0;
    let mut synth = cola::ColaSynth::with_seed(nfft, 0.5, fs, 42);
    let target = vec![s0; nfft / 2 + 1];

    let mut stream = Vec::new();
    for _ in 0..40_000 {
        stream.extend_from_slice(&synth.next_frame(&target));
    }
    let stream = &stream[nfft..];
    let (freqs, psd) = spectrum::psd_welch(stream, fs, nfft, 0.5);

    let mut sum = 0.0;
    let mut cnt = 0;
    for (k, &f) in freqs.iter().enumerate() {
        if f > 200.0 && f < fs / 2.0 - 200.0 {
            sum += psd[k];
            cnt += 1;
        }
    }
    let mean_psd = sum / cnt as f64;
    let ratio = mean_psd / s0;
    assert!(
        (ratio - 1.0).abs() < 0.03,
        "Welch PSD interior ratio {ratio} outside tightened tolerance"
    );
}

#[test]
fn cola_edge_bins_are_documented_limitation() {
    // Pins the DC/Nyquist band-edge behaviour as a DOCUMENTED limitation so it can
    // never silently regress or silently "get fixed" with a magic constant. See the
    // `next_frame` "Edge-bin accuracy" doc.
    //
    //  - DC (bin 0) is unobservable through psd_welch (per-segment mean removed), so
    //    its recovered density is essentially zero regardless of the synthesized DC
    //    power. We assert it reads LOW, documenting that callers cannot control DC.
    //  - Nyquist (last bin) carries a window-dependent bias; with the density-matched
    //    edge draw it recovers materially better than the old s*C/2 draw (which read
    //    ~0.36) but is still not power-accurate. We assert it is in a plausible band,
    //    NOT that it equals target - that is the point of the limitation.
    let fs = 4096.0;
    let nfft = 256;
    let s0 = 3.0;
    let mut synth = cola::ColaSynth::with_seed(nfft, 0.5, fs, 4242);
    let target = vec![s0; nfft / 2 + 1];

    let mut stream = Vec::new();
    for _ in 0..40_000 {
        stream.extend_from_slice(&synth.next_frame(&target));
    }
    let stream = &stream[nfft..];
    let (_freqs, psd) = spectrum::psd_welch(stream, fs, nfft, 0.5);

    let dc_ratio = psd[0] / s0;
    let nyq_ratio = psd[psd.len() - 1] / s0;
    // DC is not controllable through the mean-removing Welch estimator: the recovered
    // bin-0 density reads far below target (only neighbour-bin leakage survives the
    // per-segment mean removal). Pin that it is well under target.
    assert!(
        dc_ratio < 0.5,
        "DC ratio {dc_ratio}: unexpectedly close to target through psd_welch"
    );
    // Nyquist: density-matched draw lifts it well above the old ~0.36 but it stays
    // below target (window-leakage residual). Guard the documented range.
    assert!(
        (0.4..1.2).contains(&nyq_ratio),
        "Nyquist ratio {nyq_ratio} outside documented band-edge range"
    );
}

#[test]
fn cola_welch_psd_matches_shaped_target_shape() {
    // SHAPE check: a two-band target (low band 10x the high band) must reproduce
    // the 10:1 ratio in the recovered PSD. This tests the per-bin magnitude draw,
    // and the band RATIO cancels the common Welch bias so it can be tight.
    let fs = 8192.0;
    let nfft = 512;
    let nbins = nfft / 2 + 1;
    let df = fs / nfft as f64;
    let mut target = vec![0.0f64; nbins];
    // low band 100..500 Hz at 10.0, high band 1500..2500 Hz at 1.0, else small floor.
    for (k, t) in target.iter_mut().enumerate() {
        let f = k as f64 * df;
        *t = if (100.0..=500.0).contains(&f) {
            10.0
        } else if (1500.0..=2500.0).contains(&f) {
            1.0
        } else {
            1e-4
        };
    }
    let mut synth = cola::ColaSynth::with_seed(nfft, 0.5, fs, 777);
    let mut stream = Vec::new();
    for _ in 0..9000 {
        stream.extend_from_slice(&synth.next_frame(&target));
    }
    let stream = &stream[nfft..];
    let (freqs, psd) = spectrum::psd_welch(stream, fs, nfft, 0.5);

    let band_mean = |lo: f64, hi: f64| {
        let mut s = 0.0;
        let mut c = 0;
        for (k, &f) in freqs.iter().enumerate() {
            if f >= lo && f <= hi {
                s += psd[k];
                c += 1;
            }
        }
        s / c as f64
    };
    // stay away from band edges to avoid Welch spectral leakage smearing.
    let low = band_mean(150.0, 450.0);
    let high = band_mean(1700.0, 2300.0);
    let ratio = low / high;
    assert!(
        (8.0..12.5).contains(&ratio),
        "shaped-target band ratio {ratio} (expected ~10)"
    );
    // and the low-band absolute is ~10 within the same Welch tolerance.
    assert!((low / 10.0 - 1.0).abs() < 0.2, "low band abs {low}");
}

#[test]
fn cola_is_deterministic_for_a_seed() {
    let fs = 4096.0;
    let nfft = 128;
    let target = vec![1.0f64; nfft / 2 + 1];
    let mut a = cola::ColaSynth::with_seed(nfft, 0.5, fs, 99);
    let mut b = cola::ColaSynth::with_seed(nfft, 0.5, fs, 99);
    for _ in 0..10 {
        let fa = a.next_frame(&target);
        let fb = b.next_frame(&target);
        assert_eq!(fa, fb);
    }
}

// ---------------------------------------------------------------------------
// FRF: h_estimate_hv
// ---------------------------------------------------------------------------

#[test]
fn h_estimate_hv_recovers_known_resonator() {
    // Drive a KNOWN 2nd-order resonator with COLA random, estimate Hv, and compare
    // to the analytic biquad transfer function evaluated in closed form (h_at),
    // which does NOT use the crate FFT. In-band (where the drive has energy and
    // |H| is not tiny) the estimate must match the analytic plant.
    let fs = 8192.0;
    let nfft = 1024;
    let plant = Biquad::resonator(600.0, 8.0, fs);

    // white-ish broadband drive so every bin is excited.
    let mut synth = cola::ColaSynth::with_seed(256, 0.5, fs, 0xF00D);
    let drive_target = vec![1.0f64; 256 / 2 + 1];
    let mut drive = Vec::new();
    while drive.len() < 60_000 {
        drive.extend_from_slice(&synth.next_frame(&drive_target));
    }
    // response = plant(drive), plus a little independent output noise so Hv (not
    // just H1) is exercised.
    let mut p = plant.clone();
    let mut noise = cola::ColaSynth::with_seed(256, 0.5, fs, 0xBEEF);
    let noise_target = vec![1e-4f64; 256 / 2 + 1];
    let mut nbuf: Vec<f64> = Vec::new();
    while nbuf.len() < drive.len() {
        nbuf.extend_from_slice(&noise.next_frame(&noise_target));
    }
    let response: Vec<f64> = drive
        .iter()
        .enumerate()
        .map(|(i, &x)| p.step(x) + nbuf[i])
        .collect();

    let hest = frf::h_estimate_hv(&drive, &response, fs, nfft);
    let df = fs / nfft as f64;

    // compare magnitude in a band around the resonance where SNR is high.
    let mut worst = 0.0f64;
    let mut checked = 0;
    for (k, &he) in hest.iter().enumerate() {
        let f = k as f64 * df;
        if (150.0..=1800.0).contains(&f) {
            let ha = plant.h_at(f, fs);
            let ma = cmag(ha);
            let me = cmag(he);
            if ma > 1e-3 {
                let rel = (me - ma).abs() / ma;
                worst = worst.max(rel);
                checked += 1;
            }
        }
    }
    assert!(checked > 10, "too few bins checked ({checked})");
    assert!(worst < 0.15, "worst in-band |H| rel error {worst}");

    // and the peak location: the estimated |H| peak bin is near 600 Hz.
    let (peak_k, _) = hest.iter().enumerate().map(|(k, &h)| (k, cmag(h))).fold(
        (0usize, 0.0f64),
        |(bk, bm), (k, m)| if m > bm { (k, m) } else { (bk, bm) },
    );
    let peak_f = peak_k as f64 * df;
    assert!(
        (peak_f - 600.0).abs() < 2.0 * df,
        "resonance peak at {peak_f} Hz"
    );
}

#[test]
fn h_estimate_hv_phase_tracks_plant() {
    // The Hv phase must follow the analytic plant phase in-band.
    let fs = 8192.0;
    let nfft = 1024;
    let plant = Biquad::resonator(400.0, 5.0, fs);
    let mut synth = cola::ColaSynth::with_seed(256, 0.5, fs, 0x5151);
    let drive_target = vec![1.0f64; 256 / 2 + 1];
    let mut drive = Vec::new();
    while drive.len() < 60_000 {
        drive.extend_from_slice(&synth.next_frame(&drive_target));
    }
    let mut p = plant.clone();
    let response: Vec<f64> = drive.iter().map(|&x| p.step(x)).collect();
    let hest = frf::h_estimate_hv(&drive, &response, fs, nfft);
    let df = fs / nfft as f64;

    let mut worst = 0.0f64;
    for (k, &he) in hest.iter().enumerate() {
        let f = k as f64 * df;
        if (100.0..=1200.0).contains(&f) {
            let ha = plant.h_at(f, fs);
            if cmag(ha) > 1e-2 {
                let pa = ha.1.atan2(ha.0);
                let pe = he.1.atan2(he.0);
                let mut d = (pe - pa).abs();
                if d > std::f64::consts::PI {
                    d = std::f64::consts::TAU - d;
                }
                worst = worst.max(d);
            }
        }
    }
    // within ~0.15 rad (~8.6 deg) in-band.
    assert!(worst < 0.15, "worst phase error {worst} rad");
}

#[test]
fn h_estimate_hv_phase_sign_pinned() {
    // Pin the phase-sign convention against an INDEPENDENT pure integer-sample-delay
    // plant: y[n] = x[n - D]. The true FRF is H(e^{jw}) = e^{-j w D}, i.e. phase
    // = -w*D (a pure, lagging, negative phase). This trips if a future FFT-sign or
    // cross-spectrum convention change negates the phase (which would drive the
    // closed loop the wrong way around resonances). Magnitude is ~1 at every bin.
    let fs = 8192.0;
    let nfft = 1024;
    let delay = 3usize; // samples

    let mut synth = cola::ColaSynth::with_seed(256, 0.5, fs, 0xD3_1A);
    let drive_target = vec![1.0f64; 256 / 2 + 1];
    let mut drive = Vec::new();
    while drive.len() < 60_000 {
        drive.extend_from_slice(&synth.next_frame(&drive_target));
    }
    // response is the drive delayed by `delay` samples (zeros before it starts).
    let mut response = vec![0.0f64; drive.len()];
    let n = drive.len();
    response[delay..n].copy_from_slice(&drive[..n - delay]);

    let hest = frf::h_estimate_hv(&drive, &response, fs, nfft);
    let df = fs / nfft as f64;

    // Check several mid-band bins: the returned phase must be NEGATIVE and equal
    // -w*D within tolerance. Pick bins low enough that -w*D has not wrapped past -pi.
    // wrap point: w*D = pi -> f = fs/(2*D). For D=3, that is ~1365 Hz; stay below.
    let mut checked = 0;
    for (k, &he) in hest.iter().enumerate() {
        let f = k as f64 * df;
        if (200.0..=1000.0).contains(&f) {
            let w = std::f64::consts::TAU * f / fs;
            let expected = -w * delay as f64; // negative, lagging
            let got = he.1.atan2(he.0);
            assert!(
                got < 0.0,
                "phase at {f} Hz is {got} (>= 0): sign convention negated"
            );
            assert!(
                (got - expected).abs() < 0.1,
                "phase at {f} Hz: got {got}, expected {expected}"
            );
            checked += 1;
        }
    }
    assert!(checked > 5, "too few bins checked ({checked})");
}

#[test]
fn h_estimate_hv_single_block_gives_coherence_one() {
    // With exactly one block (drive length in [nfft, 2*nfft)), the cross-spectrum
    // satisfies |Gxy| == sqrt(Gxx*Gyy) identically (coherence 1) - the degenerate
    // no-averaging case the docstring warns about. The function still returns a
    // plausible finite result. Pin that it returns the correct length and finite
    // values (no NaN/inf), documenting the contract.
    let fs = 4096.0;
    let nfft = 256;
    let mut synth = cola::ColaSynth::with_seed(nfft, 0.5, fs, 0x1B10);
    let drive_target = vec![1.0f64; nfft / 2 + 1];
    // exactly one block: length in [nfft, 2*nfft) -> only one 50%-overlap segment.
    let mut drive = Vec::new();
    while drive.len() < nfft + nfft / 2 {
        drive.extend_from_slice(&synth.next_frame(&drive_target));
    }
    drive.truncate(nfft + 10); // in [nfft, 2*nfft): one segment only
    let plant = Biquad::resonator(500.0, 5.0, fs);
    let mut p = plant.clone();
    let response: Vec<f64> = drive.iter().map(|&x| p.step(x)).collect();

    let hest = frf::h_estimate_hv(&drive, &response, fs, nfft);
    assert_eq!(hest.len(), nfft / 2 + 1);
    assert!(
        hest.iter()
            .all(|&(re, im)| re.is_finite() && im.is_finite()),
        "single-block Hv produced non-finite values"
    );
}

#[test]
fn h_estimate_hv_unequal_length_truncates() {
    // Unequal-length drive/response (both >= nfft) are truncated to the shorter,
    // per the min-length contract. Assert it does not panic and returns the right
    // length; with response = plant(drive) truncation still recovers the plant.
    let fs = 4096.0;
    let nfft = 256;
    let mut synth = cola::ColaSynth::with_seed(nfft, 0.5, fs, 0x2C20);
    let drive_target = vec![1.0f64; nfft / 2 + 1];
    let mut drive = Vec::new();
    while drive.len() < 20_000 {
        drive.extend_from_slice(&synth.next_frame(&drive_target));
    }
    let plant = Biquad::resonator(400.0, 5.0, fs);
    let mut p = plant.clone();
    let mut response: Vec<f64> = drive.iter().map(|&x| p.step(x)).collect();
    // make response longer than drive: the extra tail must be ignored (min-length).
    response.extend(std::iter::repeat_n(0.0, 5000));

    let hest = frf::h_estimate_hv(&drive, &response, fs, nfft);
    assert_eq!(hest.len(), nfft / 2 + 1);
    // spot-check the magnitude near resonance is finite and nonzero.
    let df = fs / nfft as f64;
    let kres = (400.0 / df).round() as usize;
    assert!(cmag(hest[kres]) > 0.0 && cmag(hest[kres]).is_finite());
}

// ---------------------------------------------------------------------------
// FRF: h_estimate_hv_with_coherence (additive API)
// ---------------------------------------------------------------------------

#[test]
fn coherence_frf_matches_plain_estimator_bit_for_bit() {
    // The additive API must return an FRF vector identical to h_estimate_hv for
    // the same inputs (same accumulators). Bit-for-bit, not just close.
    let fs = 8192.0;
    let nfft = 1024;
    let plant = Biquad::resonator(500.0, 6.0, fs);
    let mut synth = cola::ColaSynth::with_seed(256, 0.5, fs, 0xC0_11);
    let drive_target = vec![1.0f64; 256 / 2 + 1];
    let mut drive = Vec::new();
    while drive.len() < 60_000 {
        drive.extend_from_slice(&synth.next_frame(&drive_target));
    }
    let mut p = plant.clone();
    let response: Vec<f64> = drive.iter().map(|&x| p.step(x)).collect();

    let plain = frf::h_estimate_hv(&drive, &response, fs, nfft);
    let (h, coh) = frf::h_estimate_hv_with_coherence(&drive, &response, fs, nfft);
    assert_eq!(h.len(), plain.len());
    assert_eq!(coh.len(), nfft / 2 + 1);
    for (k, (&a, &b)) in plain.iter().zip(h.iter()).enumerate() {
        // Identical accumulation path: expect exact equality except where the
        // plain estimator returned (0,0) on a zero-Gxx bin (the coherence variant
        // additionally zeroes zero-Gyy bins, which cannot differ here since a
        // driven plant response has energy wherever the drive does).
        assert_eq!(a, b, "bin {k}: FRF differs between estimators");
    }
}

#[test]
fn coherence_is_near_one_for_a_clean_linear_plant() {
    // A noise-free linear plant driven broadband: the response is fully explained
    // by the drive in-band, so coherence ~= 1 wherever the drive has real energy.
    let fs = 8192.0;
    let nfft = 1024;
    let plant = Biquad::resonator(500.0, 6.0, fs);
    let mut synth = cola::ColaSynth::with_seed(256, 0.5, fs, 0xC1_EA);
    let drive_target = vec![1.0f64; 256 / 2 + 1];
    let mut drive = Vec::new();
    while drive.len() < 120_000 {
        drive.extend_from_slice(&synth.next_frame(&drive_target));
    }
    let mut p = plant.clone();
    let response: Vec<f64> = drive.iter().map(|&x| p.step(x)).collect();

    let (_, coh) = frf::h_estimate_hv_with_coherence(&drive, &response, fs, nfft);
    let df = fs / nfft as f64;
    let mut checked = 0;
    for (k, &c) in coh.iter().enumerate() {
        let f = k as f64 * df;
        if (100.0..=2000.0).contains(&f) {
            assert!(
                (0.0..=1.0).contains(&c),
                "coherence {c} at {f} Hz out of [0,1]"
            );
            assert!(c > 0.95, "clean-plant coherence {c} at {f} Hz not ~1");
            checked += 1;
        }
    }
    assert!(checked > 20, "too few bins checked ({checked})");
}

#[test]
fn added_output_noise_lowers_coherence_predictably() {
    // Adding independent output noise n to the response y = H*x + n drops coherence
    // in a band toward the theoretical gamma^2 = Gyy_signal / (Gyy_signal + Gnn).
    // Concretely: more noise => lower coherence, and a heavily-noised bin is well
    // below a clean bin at the same frequency. We compare a clean run to a noisy
    // run and require the noisy coherence to be materially lower where the plant
    // gain is modest (so the noise is not swamped by signal).
    let fs = 8192.0;
    let nfft = 1024;
    let plant = Biquad::resonator(500.0, 6.0, fs);
    let mut synth = cola::ColaSynth::with_seed(256, 0.5, fs, 0xC2_AA);
    let drive_target = vec![1.0f64; 256 / 2 + 1];
    let mut drive = Vec::new();
    while drive.len() < 120_000 {
        drive.extend_from_slice(&synth.next_frame(&drive_target));
    }
    let mut p = plant.clone();
    let clean: Vec<f64> = drive.iter().map(|&x| p.step(x)).collect();

    // independent broadband output noise, comparable in scale to the off-resonance
    // response so it bites away from the resonance peak.
    let mut noise = cola::ColaSynth::with_seed(256, 0.5, fs, 0xC2_BB);
    let noise_target = vec![5e-3f64; 256 / 2 + 1];
    let mut nbuf: Vec<f64> = Vec::new();
    while nbuf.len() < clean.len() {
        nbuf.extend_from_slice(&noise.next_frame(&noise_target));
    }
    let noisy: Vec<f64> = clean
        .iter()
        .zip(nbuf.iter())
        .map(|(&y, &n)| y + n)
        .collect();

    let (_, coh_clean) = frf::h_estimate_hv_with_coherence(&drive, &clean, fs, nfft);
    let (_, coh_noisy) = frf::h_estimate_hv_with_coherence(&drive, &noisy, fs, nfft);
    let df = fs / nfft as f64;

    // Off-resonance bins (well away from 500 Hz) where the plant gain is small:
    // there the added noise dominates and coherence should drop clearly.
    let mut dropped = 0;
    let mut worst_noisy = 1.0f64;
    for (k, (&cc, &cn)) in coh_clean.iter().zip(coh_noisy.iter()).enumerate() {
        let f = k as f64 * df;
        if (1200.0..=3000.0).contains(&f) {
            assert!(
                (0.0..=1.0).contains(&cn),
                "noisy coherence {cn} out of range"
            );
            if cn < cc - 0.05 {
                dropped += 1;
            }
            worst_noisy = worst_noisy.min(cn);
        }
    }
    assert!(
        dropped > 10,
        "output noise did not lower coherence at enough off-resonance bins ({dropped})"
    );
    assert!(
        worst_noisy < 0.9,
        "worst noisy-band coherence {worst_noisy} not materially below 1"
    );
}

#[test]
fn zero_drive_bins_have_zero_coherence() {
    // A bin with exactly no drive energy (Gxx == 0) must report coherence 0
    // (untrusted) AND FRF (0,0), matching h_estimate_hv's zero-drive contract.
    // To hit Gxx == 0 EXACTLY (not just "small" - a band-limited random drive
    // still leaks window energy into out-of-band bins), the drive is identically
    // zero while the response carries independent noise: every bin has Gxx == 0,
    // so every bin is untrusted regardless of how energetic the response is. This
    // is the disconnected-drive / dead-actuator degenerate the guard exists for.
    let fs = 8192.0;
    let nfft = 1024;
    let n = 120_000usize;
    let drive = vec![0.0f64; n];
    // response = pure independent noise (no linear relationship to the zero drive).
    let mut noise = cola::ColaSynth::with_seed(256, 0.5, fs, 0xC3_00);
    let noise_target = vec![1e-2f64; 256 / 2 + 1];
    let mut response: Vec<f64> = Vec::new();
    while response.len() < n {
        response.extend_from_slice(&noise.next_frame(&noise_target));
    }
    response.truncate(n);

    let (h, coh) = frf::h_estimate_hv_with_coherence(&drive, &response, fs, nfft);
    assert_eq!(coh.len(), nfft / 2 + 1);
    for (k, &c) in coh.iter().enumerate() {
        assert_eq!(c, 0.0, "coherence at zero-drive bin {k} was {c}");
        assert_eq!(h[k], (0.0, 0.0), "FRF at zero-drive bin {k} was nonzero");
    }
}

#[test]
fn coherence_short_input_returns_zeros() {
    // Fewer than one full block: both outputs are the zero-length-safe defaults.
    let fs = 4096.0;
    let nfft = 256;
    let drive = vec![0.1f64; nfft / 2]; // shorter than nfft
    let response = vec![0.1f64; nfft / 2];
    let (h, coh) = frf::h_estimate_hv_with_coherence(&drive, &response, fs, nfft);
    assert_eq!(h.len(), nfft / 2 + 1);
    assert_eq!(coh.len(), nfft / 2 + 1);
    assert!(h.iter().all(|&(re, im)| re == 0.0 && im == 0.0));
    assert!(coh.iter().all(|&c| c == 0.0));
}

// ---------------------------------------------------------------------------
// FRF: h_inverse_regularized
// ---------------------------------------------------------------------------

#[test]
fn h_inverse_times_h_is_unity_in_band() {
    // For a well-conditioned plant, H * Hinv ~= 1 where |H| is not tiny.
    let fs = 8192.0;
    let nfft = 1024;
    let nbins = nfft / 2 + 1;
    let df = fs / nfft as f64;
    let plant = Biquad::resonator(500.0, 6.0, fs);

    let h: Vec<(f64, f64)> = (0..nbins).map(|k| plant.h_at(k as f64 * df, fs)).collect();
    let hmax2 = h
        .iter()
        .map(|&(re, im)| re * re + im * im)
        .fold(0.0, f64::max);
    // epsilon = (beta*Hmax)^2 with a small beta so in-band it is negligible.
    let eps = (0.02 * hmax2.sqrt()).powi(2);
    let hinv = frf::h_inverse_regularized(&h, eps);

    let mut worst = 0.0f64;
    for (k, (&hk, &hik)) in h.iter().zip(hinv.iter()).enumerate() {
        let f = k as f64 * df;
        if (100.0..=1500.0).contains(&f) && cmag(hk) > 0.2 * hmax2.sqrt() {
            let prod = cmul(hk, hik);
            let err = ((prod.0 - 1.0).powi(2) + prod.1.powi(2)).sqrt();
            worst = worst.max(err);
        }
    }
    assert!(
        worst < 0.05,
        "H*Hinv deviates from unity in-band by {worst}"
    );
}

#[test]
fn h_inverse_is_bounded_at_antiresonance() {
    // Design an FRF with a deep NOTCH (anti-resonance, |H| -> ~0) and confirm the
    // regularized inverse does NOT blow up. The unregularized inverse would be
    // ~1/|H| -> huge; the Tikhonov inverse must cap at 1/(2*sqrt(eps)).
    let nbins = 200;
    let mut h = vec![(0.0f64, 0.0f64); nbins];
    for (k, hk) in h.iter_mut().enumerate() {
        // magnitude 1.0 everywhere except a sharp notch at bin 100 going to 1e-6.
        let mag = if k == 100 {
            1e-6
        } else if (95..=105).contains(&k) {
            0.01 * (k as f64 - 100.0).abs()
        } else {
            1.0
        };
        // arbitrary phase ramp.
        let ph = 0.03 * k as f64;
        *hk = (mag * ph.cos(), mag * ph.sin());
    }
    let eps = 1e-2_f64.powi(2); // beta=0.01 relative to unit peak -> eps=1e-4
    let hinv = frf::h_inverse_regularized(&h, eps);

    // theoretical bound on |Hinv|: max at |H|=sqrt(eps) -> 1/(2 sqrt(eps)).
    let bound = 1.0 / (2.0 * eps.sqrt());
    let mut worst = 0.0f64;
    for &hik in &hinv {
        worst = worst.max(cmag(hik));
    }
    assert!(worst.is_finite(), "Hinv not finite");
    assert!(
        worst <= bound * 1.001,
        "Hinv magnitude {worst} exceeds Tikhonov bound {bound}"
    );
    // and specifically at the notch bin, it is bounded (not ~1e6).
    assert!(cmag(hinv[100]) <= bound * 1.001);
    // sanity: away from the notch, |Hinv| ~ 1/|H| ~ 1.
    assert!((cmag(hinv[10]) - 1.0).abs() < 0.01);
}

#[test]
fn suggest_epsilon_matches_formula() {
    let h = vec![(3.0, 4.0), (1.0, 0.0), (0.0, 0.5)]; // |H| = 5, 1, 0.5; peak^2=25
    let beta = 0.1;
    let eps = frf::suggest_epsilon(&h, beta);
    assert!((eps - (beta * beta * 25.0)).abs() < 1e-12);
}

// ---------------------------------------------------------------------------
// sine::TrackingFilter
// ---------------------------------------------------------------------------

#[test]
fn tracking_filter_locks_fixed_tone_amplitude_and_phase() {
    // Feed a pure tone A*sin(2 pi f t + theta) and confirm the lock-in recovers A
    // and theta once settled. Reference is the analytic (A, theta) of the tone.
    let fs = 20_000.0;
    let f = 137.0;
    let amp = 2.5;
    let theta = 0.6;
    let mut tf = sine::TrackingFilter::new(fs, 15.0);

    let mut last = (0.0, 0.0);
    let n = 40_000;
    for i in 0..n {
        let t = i as f64 / fs;
        let x = amp * (std::f64::consts::TAU * f * t + theta).sin();
        last = tf.update(x, f);
    }
    let (a_est, ph_est) = last;
    assert!((a_est - amp).abs() / amp < 0.02, "amp {a_est} vs {amp}");
    // phase: the filter references sin(phi); response is sin(phi_true+theta) where
    // phi_true advances at the same f but the filter's phi started at 0 with the
    // signal also effectively starting at phase theta at t=0, so the recovered
    // phase should be ~theta (mod wrap). Compare on the unit circle.
    let dphi = (ph_est - theta).rem_euclid(std::f64::consts::TAU);
    let dwrap = dphi.min(std::f64::consts::TAU - dphi);
    assert!(
        dwrap < 0.05,
        "phase {ph_est} vs {theta} (wrap dist {dwrap})"
    );
}

#[test]
fn tracking_filter_rejects_out_of_band_tone() {
    // A tone at a DIFFERENT frequency than the reference should be attenuated: the
    // lock-in only reports energy at the instantaneous reference frequency.
    let fs = 20_000.0;
    let f_ref = 200.0;
    let f_other = 600.0;
    let mut tf = sine::TrackingFilter::new(fs, 10.0);
    let mut last = (0.0, 0.0);
    for i in 0..60_000 {
        let t = i as f64 / fs;
        let x = 3.0 * (std::f64::consts::TAU * f_other * t).sin();
        last = tf.update(x, f_ref);
    }
    // reported amplitude at f_ref should be small (the 400 Hz separation is well
    // outside the 10 Hz LP band).
    assert!(last.0 < 0.15, "out-of-band leakage amplitude {}", last.0);
}

#[test]
fn tracking_filter_tracks_slow_sweep() {
    // Sweep the tone slowly and confirm the filter follows the (constant here)
    // amplitude as the instantaneous frequency changes. Amplitude reference is the
    // known constant tone amplitude.
    let fs = 20_000.0;
    let amp = 1.7;
    let f0 = 100.0;
    let f1 = 800.0;
    let dur = 4.0; // seconds, slow sweep
    let bw = 25.0;
    let mut tf = sine::TrackingFilter::new(fs, bw);

    let n = (fs * dur) as usize;
    let mut phase = 0.0f64;
    let mut max_err = 0.0f64;
    // warmup samples before we start checking (filter settle).
    let warmup = (fs * 0.3) as usize;
    for i in 0..n {
        let frac = i as f64 / n as f64;
        let f_inst = f0 + (f1 - f0) * frac; // linear sweep
                                            // generate the true signal at its own accumulated phase.
        phase += std::f64::consts::TAU * f_inst / fs;
        let x = amp * phase.sin();
        let (a_est, _) = tf.update(x, f_inst);
        if i > warmup {
            max_err = max_err.max((a_est - amp).abs() / amp);
        }
    }
    // amplitude tracked within a few percent across the whole sweep.
    assert!(max_err < 0.08, "sweep amplitude tracking error {max_err}");
}

// ---------------------------------------------------------------------------
// sine::AmplitudeServo
// ---------------------------------------------------------------------------

/// Simple memoryless plant: measured = gain * drive_scale. The servo must find
/// drive_scale = target/gain.
fn run_servo_to_convergence(
    target: f64,
    gain: f64,
    start_scale: f64,
    max_rate: f64,
    feedforward: Option<f64>,
    steps: usize,
    dt: f64,
) -> (f64, f64, bool) {
    let mut servo = sine::AmplitudeServo::new(target, max_rate);
    if let Some(g) = feedforward {
        servo.set_feedforward_from_gain(g);
    } else {
        servo.set_feedforward_scale(start_scale);
    }
    let mut scale = servo.drive_scale();
    let mut diverged = false;
    let mut last_meas = 0.0;
    for _ in 0..steps {
        let measured = gain * scale;
        last_meas = measured;
        scale = servo.update(measured, dt);
        if !scale.is_finite() || scale > 1e9 {
            diverged = true;
            break;
        }
    }
    (scale, last_meas, diverged)
}

#[test]
fn servo_converges_from_below() {
    // Start with far-too-small drive; servo must ramp up to target.
    let target = 5.0;
    let gain = 2.0; // plant: measured = 2 * scale; on-target scale = 2.5
                    // 48 dB span to cover at 6 dB/s with dt=1ms => rate-limited; needs ~14k steps.
    let (scale, meas, diverged) =
        run_servo_to_convergence(target, gain, 0.01, 6.0, None, 20000, 0.001);
    assert!(!diverged, "servo diverged from below");
    assert!(
        (meas - target).abs() / target < 0.02,
        "converged meas {meas}"
    );
    assert!(
        (scale - target / gain).abs() / (target / gain) < 0.02,
        "scale {scale}"
    );
}

#[test]
fn servo_converges_from_above() {
    // Start with far-too-large drive; servo must ramp DOWN to target (dB-rate
    // limited) without undershoot runaway.
    let target = 5.0;
    let gain = 2.0;
    let (scale, meas, diverged) =
        run_servo_to_convergence(target, gain, 100.0, 6.0, None, 20000, 0.001);
    assert!(!diverged, "servo diverged from above");
    assert!(
        (meas - target).abs() / target < 0.02,
        "converged meas {meas}"
    );
    assert!((scale - target / gain).abs() / (target / gain) < 0.02);
}

#[test]
fn servo_respects_db_rate_limit() {
    // With a large initial error, the per-step dB move must never exceed
    // max_rate * dt. Track the max observed level jump.
    let target = 10.0;
    let gain = 0.1; // needs scale = 100; huge initial error from scale=1
    let max_rate = 3.0; // dB/s
    let dt = 0.01;
    let mut servo = sine::AmplitudeServo::new(target, max_rate);
    let mut prev_scale = servo.drive_scale();
    let mut max_db_jump = 0.0f64;
    for _ in 0..3000 {
        let measured = gain * prev_scale;
        let scale = servo.update(measured, dt);
        let db_jump = (20.0 * (scale / prev_scale).log10()).abs();
        max_db_jump = max_db_jump.max(db_jump);
        prev_scale = scale;
    }
    // the max per-step move must be within the rate limit (+ tiny float slack).
    assert!(
        max_db_jump <= max_rate * dt * 1.0001,
        "db jump {max_db_jump} exceeds limit {}",
        max_rate * dt
    );
    // and it still reached target eventually.
    assert!((gain * prev_scale - target).abs() / target < 0.05);
}

#[test]
fn servo_feedforward_starts_near_target() {
    // With H^-1 feedforward (known plant gain), the servo should start essentially
    // on target and only trim.
    let target = 8.0;
    let gain = 4.0; // on-target scale = 2.0
    let mut servo = sine::AmplitudeServo::new(target, 6.0);
    servo.set_feedforward_from_gain(gain);
    let start_scale = servo.drive_scale();
    // first measurement with the feedforward drive:
    let first_meas = gain * start_scale;
    assert!(
        (first_meas - target).abs() / target < 1e-6,
        "feedforward first measurement {first_meas} not on target"
    );
    // one update should keep it on target.
    let scale = servo.update(first_meas, 0.001);
    assert!((gain * scale - target).abs() / target < 0.001);
}

#[test]
fn servo_never_diverges_on_noisy_plant() {
    // Feed the servo a noisy, occasionally-zero measurement and confirm it stays
    // bounded (no runaway) and roughly holds target. This stresses the floor on
    // the measurement and the rate limiter.
    let target = 3.0;
    let gain = 1.5;
    let mut servo = sine::AmplitudeServo::new(target, 4.0);
    servo.set_feedforward_from_gain(gain);
    let mut scale = servo.drive_scale();
    // deterministic pseudo-noise.
    let mut z = 0xDEAD_BEEFu64;
    let mut rng = || {
        z ^= z << 13;
        z ^= z >> 7;
        z ^= z << 17;
        (z >> 40) as f64 / (1u64 << 24) as f64 // in [0,1)
    };
    let mut worst_scale = scale;
    for i in 0..5000 {
        let noise = 1.0 + 0.3 * (rng() - 0.5);
        // every 200th sample, simulate a dropout (measured ~ 0).
        let measured = if i % 200 == 0 {
            1e-9
        } else {
            gain * scale * noise
        };
        scale = servo.update(measured, 0.001);
        assert!(scale.is_finite(), "scale went non-finite at step {i}");
        worst_scale = worst_scale.max(scale);
    }
    // stayed bounded well under any runaway threshold.
    assert!(
        worst_scale < 100.0 * (target / gain),
        "servo scale grew unbounded: {worst_scale}"
    );
    // and the mean drive is in the right neighborhood.
    assert!(
        (gain * scale / target).ln().abs() < 0.5,
        "final off target: scale {scale}"
    );
}

#[test]
fn servo_holds_drive_on_sustained_dropout_no_runaway() {
    // CRITICAL safety: a sustained zero/near-zero measurement (control-accel cable
    // disconnect - the most common field fault) must NOT ramp the drive up. The
    // fault-hold must freeze the drive and latch the fault, and the absolute ceiling
    // must bound it regardless. Without the fix the drive ramps at max_rate forever.
    let target = 10.0;
    let mut servo = sine::AmplitudeServo::new(target, 12.0).with_drive_ceiling(50.0);
    // seed a sensible starting drive first (as a real startup would).
    servo.set_feedforward_from_gain(2.0); // scale = target/gain = 5.0
    let start = servo.drive_scale();

    // Now the control accelerometer "disconnects": measured reads ~0 forever.
    let dt = 0.001;
    let mut scale = servo.drive_scale();
    for _ in 0..20_000 {
        scale = servo.update(0.0, dt); // stuck-at-zero
    }
    // Drive must be frozen at the seed (not ramped up), and the fault latched.
    assert!(servo.faulted(), "servo did not latch the dropout fault");
    assert!(
        (scale - start).abs() < 1e-9,
        "drive ramped on sustained dropout: {start} -> {scale}"
    );
    // And even if it had ramped, the ceiling caps it hard.
    assert!(
        scale <= servo.max_drive_scale() + 1e-9,
        "drive exceeded ceiling {}",
        servo.max_drive_scale()
    );
}

#[test]
fn servo_absolute_ceiling_bounds_drive() {
    // The ceiling is a VALUE bound, independent of the rate limit. Command a
    // sustained positive error (plausible, below target but above the fault floor)
    // and confirm the drive saturates at the ceiling and never exceeds it.
    let target = 100.0;
    let ceiling = 10.0; // +20 dB
    let mut servo = sine::AmplitudeServo::new(target, 100.0).with_drive_ceiling(ceiling);
    let dt = 0.01;
    let mut scale = servo.drive_scale();
    for _ in 0..100_000 {
        // Keep the measurement plausible (above target*fault_fraction) but far below
        // target so the servo always wants more drive. It must stop at the ceiling.
        let measured = (target * 0.05).max(target * 2e-3);
        scale = servo.update(measured, dt);
        assert!(
            scale <= ceiling + 1e-9,
            "drive {scale} exceeded ceiling {ceiling}"
        );
    }
    // it should have climbed to the ceiling.
    assert!((scale - ceiling).abs() < 1e-6, "did not saturate: {scale}");
}

#[test]
fn servo_pi_dynamics_bounded_overshoot_no_clamp() {
    // With the rate limiter effectively inactive (huge max_rate) and a modest error,
    // convergence is driven by kp/ki, not the clamp. This exercises the PI dynamics
    // the rate-limited tests do not: assert it converges and overshoot is bounded.
    // Instantaneous-gain plant -> the loop should NOT overshoot appreciably.
    let target = 5.0;
    let gain = 2.0; // on-target scale = 2.5
    let mut servo = sine::AmplitudeServo::new(target, 1e6); // clamp inactive
    servo.set_feedforward_scale(1.0); // start at scale 1 (below target)
    let dt = 0.001;
    let mut scale = servo.drive_scale();
    let mut max_meas = 0.0f64;
    let mut settled_at = None;
    for i in 0..5000 {
        let measured = gain * scale;
        max_meas = max_meas.max(measured);
        scale = servo.update(measured, dt);
        if settled_at.is_none() && ((gain * scale - target).abs() / target) < 0.01 {
            settled_at = Some(i);
        }
    }
    // converged.
    let final_meas = gain * scale;
    assert!(
        (final_meas - target).abs() / target < 0.01,
        "PI did not converge: {final_meas}"
    );
    // overshoot on an instantaneous-gain plant must be small (a few percent), which
    // confirms kp/ki are sane and anti-windup is not injecting a kick.
    let overshoot = (max_meas - target).max(0.0) / target;
    assert!(
        overshoot < 0.10,
        "excess overshoot {overshoot} (max {max_meas})"
    );
    assert!(settled_at.is_some(), "never settled");
}

#[test]
fn servo_handles_variable_dt() {
    // Real engines feed a jittery dt. Both ki*e*dt and the rate clamp scale with dt;
    // confirm the loop still converges and holds target under varying dt.
    let target = 4.0;
    let gain = 1.5;
    let mut servo = sine::AmplitudeServo::new(target, 20.0);
    servo.set_feedforward_scale(0.5);
    let mut scale = servo.drive_scale();
    // deterministic dt jitter around 1 ms.
    let mut z = 0x51D_u64;
    let mut jitter = || {
        z ^= z << 13;
        z ^= z >> 7;
        z ^= z << 17;
        let u = (z >> 40) as f64 / (1u64 << 24) as f64; // [0,1)
        0.0005 + 0.001 * u // dt in [0.5ms, 1.5ms)
    };
    for _ in 0..20_000 {
        let measured = gain * scale;
        let dt = jitter();
        scale = servo.update(measured, dt);
        assert!(scale.is_finite());
    }
    let meas = gain * scale;
    assert!(
        (meas - target).abs() / target < 0.02,
        "variable-dt servo off target: {meas}"
    );
}
