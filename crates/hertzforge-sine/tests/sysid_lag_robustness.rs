//! The sysID lag measurement against the chassis-observed failure shape.
//!
//! The second chassis sitting (2026-08-08) printed the first align notes from
//! real iron: `lag 0 frames, peak |r| 0.040, searched 2048 lags` while the
//! WINDOWED coherence on the same pair read 0.990. A response that correlates
//! per-bin but not in broadband time is a normalization diluter: energy in the
//! response that the drive does not carry (DC offset, drift, out-of-band noise)
//! inflates `r_rms` and drowns the true peak; a ramped burst head can
//! additionally make the head-anchored window degenerate. This test builds
//! exactly that shape - known lag, DC-diluted response, ramped head - and
//! requires the measurement to find the lag anyway.
//!
//! OBSERVED RED 2026-08-08 against the pre-fix implementation (head-anchored
//! window, no mean removal): lag 0, peak 0.089 - the chassis signature.

use hertzforge_sine::service::measured_pipeline_lag_for_tests as measured_lag;

/// Deterministic white noise, no rand dep. Index-hashed (splitmix-style
/// avalanche over `k XOR seed-constant`), NOT a stepped LCG: two seeds of one
/// LCG are two positions on the SAME orbit, so "independent" streams are a
/// pure time shift of each other and the second test's uncorrelated premise
/// silently fails (observed here first try: |r| 0.765 between "unrelated"
/// streams - the instrument was right, the fixture was wrong).
fn noise(n: usize, seed: u64) -> Vec<f64> {
    let c = seed.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    (0..n)
        .map(|k| {
            let mut z = (k as u64) ^ c;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            // 32 random bits over [0, 2^32) / 2^31 - 1 => uniform [-1, 1).
            // The first cut shifted 33 and produced [-1, 0): a -0.5 DC on BOTH
            // streams, which the pre-fix scorer rated |r| 0.75 at every lag -
            // an accidental, perfect demonstration of the DC-dilution class
            // this file exists to pin.
            ((z >> 32) as f64 / (1u64 << 31) as f64) - 1.0
        })
        .collect()
}

#[test]
fn finds_the_lag_through_dc_dilution_and_a_ramped_head() {
    let fs = 51_200.0;
    let n = 262_144;
    let true_lag = 1_449usize; // the Random-measured pipeline figure

    // BAND-LIMITED noise - RANDOM's premise, kept as coverage that the
    // white-burst fix below does not break the band-limited case. CORRECTED
    // 2026-08-08: this comment previously called a white fixture "a fixture
    // lie about the plant" - the REAL sine burst is white to Nyquist
    // (run_sysid's bare xorshift, no filter), and the chassis proved it
    // (r_rms/d_rms 0.883 = the full-band ZOH loss). The test had encoded the
    // bug; the white cases below now carry the chassis shape.
    let white = noise(n + 64, 0x5EED);
    let taps = 51usize;
    let mut drive: Vec<f64> = (0..n)
        .map(|k| white[k..k + taps].iter().sum::<f64>() / taps as f64)
        .collect();
    // Burst ramp-in: the first 8192 frames fade up, so a head-anchored
    // correlation window sees mostly-attenuated drive.
    for (k, d) in drive.iter_mut().take(8192).enumerate() {
        *d *= k as f64 / 8192.0;
    }

    // Response = drive delayed by the true lag, plus a DC offset ten times the
    // signal RMS (the diluter class; a jumpered AI with offset reads exactly
    // this shape).
    let rms = (drive.iter().map(|x| x * x).sum::<f64>() / n as f64).sqrt();
    let dc = 10.0 * rms;
    let mut resp = vec![dc; n];
    for k in true_lag..n {
        resp[k] += drive[k - true_lag];
    }

    let (lag, peak, searched, diag) = measured_lag(&drive, &resp, fs);
    assert!(searched > 0, "search must have run (diag: {diag})");
    assert!(
        peak >= 0.2,
        "the true peak must clear the trust floor through the DC dilution; \
         got peak {peak:.3} at lag {lag} (diag: {diag})"
    );
    // Single-frame stride by construction (the band parameter is GONE - the
    // caller-shape defect is unrepresentable). One frame of tolerance for the
    // window edge.
    assert!(
        (lag as i64 - true_lag as i64).abs() <= 1,
        "lag {lag} vs true {true_lag} (peak {peak:.3}, diag: {diag})"
    );
}

#[test]
fn a_genuinely_uncorrelated_pair_stays_untrusted() {
    // The other half of the contract (the gate's whole job per the source
    // comment): noise vs UNRELATED noise must not clear the trust floor -
    // mean removal must not manufacture confidence where there is none.
    let n = 262_144;
    let drive = noise(n, 0x5EED);
    let resp = noise(n, 0xD1FF);
    let (_lag, peak, _searched, diag) = measured_lag(&drive, &resp, 51_200.0);
    assert!(
        peak < 0.2,
        "unrelated noise must stay below the trust floor; got {peak:.3} (diag: {diag})"
    );
}

/// The PRODUCTION burst shape: white to Nyquist (run_sysid's xorshift at
/// sysid scale), response = 2-tap ZOH resample at an OFF-GRID lag + the
/// chassis-logged DC. This is the fixture the chassis lines forced:
/// `lag 0 frames, peak |r| 0.043, searched 2048 lags; NO-PEAK` at coherence
/// 0.990 (two runs, 2026-08-08). The old stride (sized
/// from band_hi as if the burst were band-limited) steps over the 2-3 frame
/// lobe ~88% of the time.
///
/// OBSERVED RED 2026-08-08 against the band_hi-strided call shape: lag 0,
/// peak ~0.04 - the chassis signature verbatim.
fn white_burst(n: usize, seed: u64) -> Vec<f64> {
    // Same statistics as run_sysid's generator: U[-scale, scale), zero-mean.
    noise(n, seed).iter().map(|u| u * 0.005).collect()
}

fn zoh_response(drive: &[f64], lag: usize, dc: f64) -> Vec<f64> {
    // The measured loopback kernel: ~2-tap ZOH reconstruction loss shape.
    let n = drive.len();
    let mut resp = vec![dc; n];
    for k in (lag + 1)..n {
        resp[k] += 0.874 * drive[k - lag] + 0.126 * drive[k - lag - 1];
    }
    resp
}

#[test]
fn the_white_production_burst_finds_an_off_grid_lag() {
    let n = 1usize << 19; // 524288: the chassis shape, CI-affordable
    let true_lag = 7_097usize; // run 2's inferred figure; 7097 % 25 == 22, off the old grid
    let drive = white_burst(n, 0x5EED);
    let resp = zoh_response(&drive, true_lag, 7.3549e-4);

    // The fixture must stay WHITE - if someone band-limits it the test stops
    // guarding the real burst (this file did exactly that once).
    let lag1: f64 = drive.windows(2).map(|w| w[0] * w[1]).sum::<f64>()
        / drive.iter().map(|x| x * x).sum::<f64>();
    assert!(
        lag1.abs() < 0.05,
        "fixture must be white (lag-1 autocorr {lag1:.3}) - a band-limited fixture encodes the bug"
    );

    let (lag, peak, _searched, diag) = measured_lag(&drive, &resp, 51_200.0);
    // The chassis instrument line's own ratio must reproduce (white + ZOH).
    let ratio = diag.r_rms / diag.d_rms;
    assert!(
        (ratio - 0.883).abs() < 0.02 * 0.883,
        "the fixture must reproduce the chassis r_rms/d_rms 0.883; got {ratio:.4}"
    );
    assert!(
        peak >= 0.2,
        "white burst: peak {peak:.3} at lag {lag} (diag {diag})"
    );
    assert!(
        (lag as i64 - true_lag as i64).abs() <= 1,
        "lag {lag} vs true {true_lag} (peak {peak:.3})"
    );
}

#[test]
fn resolution_holds_across_a_full_stride_of_lags() {
    // The row that names the defect as RESOLUTION, not range: every lag in a
    // 25-frame window must resolve, so a regression can never pass by landing
    // on a grid multiple.
    let n = 1 << 18; // 262144
    let drive = white_burst(n, 0xB00C);
    for true_lag in 7_095..7_120usize {
        let resp = zoh_response(&drive, true_lag, 0.0);
        let (lag, peak, _s, _d) = measured_lag(&drive, &resp, 51_200.0);
        assert!(
            peak >= 0.2 && (lag as i64 - true_lag as i64).abs() <= 1,
            "lag {true_lag}: found {lag} at peak {peak:.3}"
        );
    }
}
