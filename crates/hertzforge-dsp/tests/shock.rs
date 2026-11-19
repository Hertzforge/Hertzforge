//! Golden-vector tests for the classical-shock + SRS DSP primitives.
//!
//! Every reference here is ANALYTIC (closed-form) or a physically-derived
//! identity, never a value copied out of the same Rust code under test, so the
//! test and the implementation cannot silently co-confirm. The standard
//! identities used:
//!
//! - Uncompensated half-sine velocity change `dV = A * 2T/pi` (integral of one
//!   sine half-lobe). In this crate's in/g output that is
//!   `G_TO_IN_S2 * A * 2T/pi`. Because a half-sine is one-signed the velocity
//!   never returns toward zero, so the residual velocity equals the peak
//!   velocity (both equal `dV`).
//! - Odd-half-cycle wavelet `sin(2*pi*f*t/N) * sin(2*pi*f*t)`, N odd, has ZERO
//!   net velocity and displacement by construction (the property SRS synthesis relies on).
//! - SDOF pseudo-velocity at resonance `PV = A_in * Q / wn` (steady-state
//!   relative-displacement gain `1/(2*zeta) = Q`).
//! - Low-frequency SRS velocity asymptote for a half-sine: undamped maximax
//!   `SRS -> 4*A*tau*f_n` (g) and `PVSS -> dV` (in/s).
//! - Fractional-octave centers are exact powers of `2^(1/N)`.

use hertzforge_dsp::integrate::{self, G_TO_IN_S2};
use hertzforge_dsp::shock::{self, pulse};
use hertzforge_dsp::srs::{self, OctaveFraction};
use std::f64::consts::PI;

// ---------------------------------------------------------------------------
// helpers (analytic pulse builders, not copied from the crate under test)
// ---------------------------------------------------------------------------

/// A half-sine base-acceleration pulse of peak `amp` (g), main-lobe duration
/// `tau` (s), sampled at `fs`, followed by `quiet` seconds of zeros so the
/// residual response and post-pulse kinematics develop fully.
fn half_sine_pulse(amp: f64, tau: f64, fs: f64, quiet: f64) -> Vec<f64> {
    let n_pulse = (tau * fs).round() as usize;
    let n_quiet = (quiet * fs).round() as usize;
    let mut v = Vec::with_capacity(n_pulse + n_quiet);
    for k in 0..n_pulse {
        let t = k as f64 / fs;
        v.push(amp * (PI * t / tau).sin());
    }
    v.extend(std::iter::repeat_n(0.0, n_quiet));
    v
}

// ---------------------------------------------------------------------------
// pulse_kinematics - the PURE (no-HP) double-integral
// ---------------------------------------------------------------------------

/// Half-sine analytic identity: an UNCOMPENSATED half-sine has velocity change
/// `dV = G_TO_IN_S2 * A * 2T/pi`, and because it is one-signed the residual
/// velocity equals the peak velocity (both == dV). Reproduced to < 0.1%.
#[test]
fn pulse_kinematics_half_sine_velocity_identity() {
    let fs = 50_000.0; // fine grid so the trapezoid integral is tight
    let amp = 10.0; // g
    let tau = 0.005; // s
    let pulse = half_sine_pulse(amp, tau, fs, 0.05);

    let k = shock::pulse_kinematics(&pulse, fs);

    // dV = A * 2T/pi in the acceleration's units*s; the crate outputs in/s, so
    // multiply the g amplitude by G_TO_IN_S2.
    let dv = G_TO_IN_S2 * amp * 2.0 * tau / PI;

    assert!(
        (k.peak_accel_g - amp).abs() / amp < 1e-3,
        "peak accel {} vs {amp} g",
        k.peak_accel_g
    );
    let rel_peak = (k.peak_velocity_ips - dv).abs() / dv;
    assert!(
        rel_peak < 1e-3,
        "peak velocity {} vs analytic dV {dv} (rel {rel_peak:.2e})",
        k.peak_velocity_ips
    );
    let rel_resid = (k.residual_velocity_ips - dv).abs() / dv;
    assert!(
        rel_resid < 1e-3,
        "residual velocity {} vs analytic dV {dv} (rel {rel_resid:.2e}) - a one-signed \
         pulse never returns to zero, so residual == peak",
        k.residual_velocity_ips
    );
    // residual == peak to float precision (both are dV).
    assert!(
        (k.residual_velocity_ips - k.peak_velocity_ips).abs() / dv < 1e-9,
        "residual {} should equal peak {} for a one-signed pulse",
        k.residual_velocity_ips,
        k.peak_velocity_ips
    );
}

/// A wavelet-like odd-half-cycle signal `sin(2*pi*f*t/N) * sin(2*pi*f*t)` (N=5)
/// has ZERO net velocity AND zero net displacement via the pure double-integral
/// (the self-nulling property SRS synthesis relies on). Both residuals are negligible
/// relative to the peak of the same quantity.
#[test]
fn pulse_kinematics_odd_half_cycle_wavelet_self_nulls() {
    let n: f64 = 5.0; // odd half-cycles
    let f: f64 = 100.0; // carrier Hz
    let fs = 50_000.0;
    // The envelope sin(2*pi*f*t/N) completes its first half-lobe (0 -> pi) over
    // t in [0, N/(2f)]; that is the wavelet's finite support.
    let dur = n / (2.0 * f);
    let npulse = (dur * fs).round() as usize;
    let mut wav: Vec<f64> = (0..npulse)
        .map(|i| {
            let t = i as f64 / fs;
            (2.0 * PI * f * t / n).sin() * (2.0 * PI * f * t).sin()
        })
        .collect();
    // a quiet tail so any residual velocity would ramp displacement visibly.
    wav.extend(std::iter::repeat_n(0.0, (0.05 * fs) as usize));

    let k = shock::pulse_kinematics(&wav, fs);

    // Residual velocity/displacement ~ 0 relative to the in-pulse peaks.
    assert!(k.peak_velocity_ips > 1e-3, "sanity: pulse has real motion");
    let rv_ratio = k.residual_velocity_ips.abs() / k.peak_velocity_ips;
    assert!(
        rv_ratio < 1e-6,
        "wavelet residual velocity {} not ~0 (ratio {rv_ratio:.2e})",
        k.residual_velocity_ips
    );
    let peak_abs_disp = k.peak_disp_in.abs().max(k.min_disp_in.abs());
    let rd_ratio = k.residual_disp_in.abs() / peak_abs_disp.max(1e-12);
    assert!(
        rd_ratio < 1e-6,
        "wavelet residual displacement {} not ~0 (ratio {rd_ratio:.2e})",
        k.residual_disp_in
    );
}

/// Empty / degenerate inputs return an all-zero PulseKinematics (edge-case
/// handling mirrors srs_analyze's empty-record contract).
#[test]
fn pulse_kinematics_empty_and_bad_fs() {
    let z = shock::pulse_kinematics(&[], 1000.0);
    assert_eq!(z.peak_accel_g, 0.0);
    assert_eq!(z.pk_pk_disp_in, 0.0);
    let z2 = shock::pulse_kinematics(&[1.0, 2.0, 3.0], 0.0);
    assert_eq!(z2.peak_velocity_ips, 0.0);
}

/// The velocity/displacement SERIES a display draws and the SCALARS the
/// feasibility gate checks are ONE integration, not two.
///
/// `pulse_kinematic_series` exists so the operator's accel/velocity/displacement
/// toggle plots the same numbers the kinematics card reports. The gate is EXACT
/// equality, not a tolerance: the scalars are extrema of these very arrays, so
/// any difference at all means a second integrator has appeared. The analytic
/// half-sine identity `dV = G_TO_IN_S2 * A * 2*tau/pi` is asserted alongside so
/// the pair cannot agree on a wrong answer.
#[test]
fn pulse_kinematic_series_is_the_same_integration_as_the_scalars() {
    let fs = 20_000.0;
    let amp = 30.0; // g
    let tau = 0.011; // s
    let pulse = half_sine_pulse(amp, tau, fs, 0.05);

    let series = shock::pulse_kinematic_series(&pulse, fs);
    let k = shock::pulse_kinematics(&pulse, fs);

    assert_eq!(series.velocity_ips.len(), pulse.len());
    assert_eq!(series.disp_in.len(), pulse.len());

    let peak_v = series
        .velocity_ips
        .iter()
        .fold(0.0f64, |m, &v| m.max(v.abs()));
    let max_d = series
        .disp_in
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    let min_d = series.disp_in.iter().cloned().fold(f64::INFINITY, f64::min);
    assert_eq!(peak_v, k.peak_velocity_ips);
    assert_eq!(
        *series.velocity_ips.last().unwrap(),
        k.residual_velocity_ips
    );
    assert_eq!(max_d, k.peak_disp_in);
    assert_eq!(min_d, k.min_disp_in);
    assert_eq!(max_d - min_d, k.pk_pk_disp_in);
    assert_eq!(*series.disp_in.last().unwrap(), k.residual_disp_in);

    // Analytic anchor: a one-signed half-sine's velocity change.
    let dv = G_TO_IN_S2 * amp * 2.0 * tau / PI;
    let rel = (peak_v - dv).abs() / dv;
    assert!(
        rel < 1e-3,
        "peak velocity {peak_v} vs analytic {dv} (rel {rel:.2e})"
    );

    // Degenerate inputs mirror the scalar contract.
    let empty = shock::pulse_kinematic_series(&[], fs);
    assert!(empty.velocity_ips.is_empty());
    let bad_fs = shock::pulse_kinematic_series(&pulse, 0.0);
    assert!(bad_fs.disp_in.is_empty());
}

// ---------------------------------------------------------------------------
// velocity_to_displacement - streaming HP-then-integrate (mirror of
// accel_to_velocity's test pattern)
// ---------------------------------------------------------------------------

/// Integrating V*sin(wt) [in/s] yields -(V/w)*cos(wt) [in] once the high-pass
/// transient settles. Same test shape as accel_to_velocity_sine_matches_analytic
/// but one stage down (in/s -> in), so no g conversion.
#[test]
fn velocity_to_displacement_sine_matches_analytic() {
    let fs = 10_000.0;
    let f = 100.0; // well above the HP corner
    let v_amp = 2.0; // in/s
    let w = 2.0 * PI * f;
    let n = 200_000; // 20 s -> HP settles early
    let vel: Vec<f64> = (0..n).map(|k| v_amp * (w * k as f64 / fs).sin()).collect();

    let hp = 2.0; // Hz, 50x below the signal -> passband gain ~1
    let disp = integrate::velocity_to_displacement(&vel, fs, hp);

    let expected_amp = v_amp / w; // V/w, no unit conversion (already in/s -> in)
    let tail_start = n - (fs as usize);
    let dmax = disp[tail_start..]
        .iter()
        .fold(0.0f64, |m, &v| m.max(v.abs()));
    let rel = (dmax - expected_amp).abs() / expected_amp;
    assert!(
        rel < 0.02,
        "displacement amplitude {dmax} vs analytic {expected_amp} (rel {rel})"
    );
}

/// The high-pass kills DC drift: a constant velocity would ramp displacement
/// forever without the HP; with it, the settled displacement is bounded. Mirror
/// of accel_to_velocity_kills_dc_drift.
#[test]
fn velocity_to_displacement_kills_dc_drift() {
    let fs = 1000.0;
    let n = 20_000;
    // constant 1 in/s -> raw integral ramps to 20 in over 20 s.
    let vel = vec![1.0f64; n];
    let hp = 1.0;
    let disp = integrate::velocity_to_displacement(&vel, fs, hp);
    let tail = &disp[n - 1000..];
    let tail_max = tail.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
    assert!(
        tail_max < 0.05,
        "HP failed to kill DC drift: tail max {tail_max} in"
    );
}

/// Chaining accel_to_velocity then velocity_to_displacement reproduces the
/// displacement amplitude of a sine acceleration: for A*sin(wt) [g],
/// displacement amplitude is G_TO_IN_S2*A/w^2 [in]. This confirms the two stages
/// compose (the streaming double-integral).
#[test]
fn accel_to_disp_chain_matches_analytic() {
    let fs = 10_000.0;
    let f = 100.0;
    let a = 2.0; // g
    let w = 2.0 * PI * f;
    let n = 200_000;
    let accel: Vec<f64> = (0..n).map(|k| a * (w * k as f64 / fs).sin()).collect();

    let hp = 2.0;
    let vel = integrate::accel_to_velocity(&accel, fs, hp);
    let disp = integrate::velocity_to_displacement(&vel, fs, hp);

    let expected = G_TO_IN_S2 * a / (w * w); // |disp| = G*A/w^2
    let tail_start = n - (fs as usize);
    let dmax = disp[tail_start..]
        .iter()
        .fold(0.0f64, |m, &v| m.max(v.abs()));
    let rel = (dmax - expected).abs() / expected;
    assert!(
        rel < 0.03,
        "chained accel->disp amplitude {dmax} vs analytic {expected} (rel {rel})"
    );
}

// ---------------------------------------------------------------------------
// octave_freqs - band-center generator
// ---------------------------------------------------------------------------

/// Adjacent centers are in exactly the 2^(1/N) ratio, the low endpoint is exact,
/// and the top of the requested band is covered (last center >= f_hi).
#[test]
fn octave_freqs_ratios_and_coverage() {
    for (frac, n) in [
        (OctaveFraction::Whole, 1.0),
        (OctaveFraction::Third, 3.0),
        (OctaveFraction::Sixth, 6.0),
        (OctaveFraction::Twelfth, 12.0),
        (OctaveFraction::Arbitrary(24.0), 24.0),
    ] {
        let f = srs::octave_freqs(10.0, 1000.0, frac);
        assert!(f.len() >= 2, "{frac:?} produced too few bands");
        // low endpoint exact
        assert!(
            (f[0] - 10.0).abs() < 1e-12,
            "{frac:?} low endpoint {}",
            f[0]
        );
        // top covered
        assert!(
            *f.last().unwrap() >= 1000.0 * (1.0 - 1e-9),
            "{frac:?} top {} does not cover 1000",
            f.last().unwrap()
        );
        // exact ratio between adjacent centers
        let ratio = 2.0_f64.powf(1.0 / n);
        for w in f.windows(2) {
            let r = w[1] / w[0];
            assert!(
                (r - ratio).abs() / ratio < 1e-12,
                "{frac:?} adjacent ratio {r} != 2^(1/{n}) = {ratio}"
            );
        }
    }
}

/// Whole-octave over an exact power-of-two interval lands exactly on the
/// endpoints and doubles each step.
#[test]
fn octave_freqs_whole_exact_endpoints() {
    let f = srs::octave_freqs(10.0, 80.0, OctaveFraction::Whole);
    assert_eq!(f.len(), 4);
    assert!((f[0] - 10.0).abs() < 1e-12);
    assert!((f[1] - 20.0).abs() < 1e-9);
    assert!((f[2] - 40.0).abs() < 1e-9);
    assert!((f[3] - 80.0).abs() < 1e-9);
}

/// Arbitrary fraction N=2 gives ratio sqrt(2); single-point interval returns one
/// frequency; bad bounds return empty.
#[test]
fn octave_freqs_arbitrary_single_and_bad() {
    let arb = srs::octave_freqs(100.0, 400.0, OctaveFraction::Arbitrary(2.0));
    // 100, 141.42, 200, 282.84, 400
    assert_eq!(arb.len(), 5);
    assert!((arb[1] / arb[0] - std::f64::consts::SQRT_2).abs() < 1e-12);
    assert!((arb.last().unwrap() - 400.0).abs() / 400.0 < 1e-9);

    let single = srs::octave_freqs(50.0, 50.0, OctaveFraction::Third);
    assert_eq!(single, vec![50.0]);

    assert!(srs::octave_freqs(-1.0, 100.0, OctaveFraction::Third).is_empty());
    assert!(srs::octave_freqs(100.0, 50.0, OctaveFraction::Third).is_empty());
    assert!(srs::octave_freqs(10.0, 100.0, OctaveFraction::Arbitrary(0.0)).is_empty());
}

// ---------------------------------------------------------------------------
// srs_analyze round-trip shape over octave_freqs (the standard SRS sanity)
// ---------------------------------------------------------------------------

/// Synthesize a half-sine, run srs_analyze at Q=10 over octave_freqs(10,1000,
/// Third), and assert the standard SRS shape: the low-frequency asymptote
/// approaches the velocity-change-dominated line SRS ~ omega_n * dV (equivalently
/// 4*A*tau*f_n in g), and the high-frequency asymptote approaches the peak input
/// acceleration.
#[test]
fn srs_analyze_half_sine_roundtrip_shape() {
    let fs = 50_000.0;
    let amp = 10.0; // g
    let tau = 0.005; // s -> primary knee near 1/tau = 200 Hz
    let pulse = half_sine_pulse(amp, tau, fs, 1.0);
    let q = 10.0;

    let freqs = srs::octave_freqs(10.0, 1000.0, OctaveFraction::Third);
    let srs = srs::srs_analyze(&pulse, fs, &freqs, q);

    // Low-frequency: SRS rises linearly with f (velocity regime). At the lowest
    // band, SRS_g should approach the undamped asymptote 4*A*tau*f_n; at Q=10 it
    // is a bit below. Check it is on that line to within ~15% (Q=10 damping
    // pulls it down; the tight version is the high-Q PVSS test below).
    let f_lo = freqs[0];
    let low_asymptote_g = 4.0 * amp * tau * f_lo;
    let rel_low = (srs[0] - low_asymptote_g).abs() / low_asymptote_g;
    assert!(
        rel_low < 0.15,
        "low-f SRS {} vs velocity asymptote {low_asymptote_g} (rel {rel_low})",
        srs[0]
    );
    // Low frequency must be well below the peak plateau.
    assert!(srs[0] < 0.3 * amp, "low-f SRS {} not small", srs[0]);

    // High-frequency: rigid-follow -> approaches the input peak within ~10%.
    let hi = *srs.last().unwrap();
    assert!(
        (hi - amp).abs() / amp < 0.1,
        "high-f SRS {hi} should approach input peak {amp}"
    );

    // Near the knee the primary SRS overshoots the input peak (half-sine ~1.7x).
    let knee_idx = freqs.iter().position(|&f| f >= 200.0).unwrap();
    assert!(
        srs[knee_idx] > amp,
        "knee SRS {} should exceed input peak {amp}",
        srs[knee_idx]
    );
}

// ---------------------------------------------------------------------------
// srs_pseudo_velocity
// ---------------------------------------------------------------------------

/// PVSS at a SDOF's natural frequency, driven by a sustained sine at that
/// frequency, shows the steady-state resonance gain: |z| = A_in/(2*zeta*wn^2),
/// so PV = wn*|z| = A_in*Q/wn. With A_in in g, the in/s PV is A_in*G_TO_IN_S2*Q/wn.
#[test]
fn srs_pseudo_velocity_resonance_gain() {
    let fs = 50_000.0;
    let fn_hz = 100.0;
    let q = 20.0;
    let amp_in = 1.0; // g
                      // many cycles so the resonant build-up reaches steady state (~Q cycles).
    let n = (fs * (3.0 * q / fn_hz)) as usize;
    let drive: Vec<f64> = (0..n)
        .map(|k| amp_in * (2.0 * PI * fn_hz * k as f64 / fs).sin())
        .collect();

    let pv = srs::srs_pseudo_velocity(&drive, fs, &[fn_hz], q)[0];
    let wn = 2.0 * PI * fn_hz;
    let expected = amp_in * G_TO_IN_S2 * q / wn;
    let rel = (pv - expected).abs() / expected;
    assert!(
        rel < 0.05,
        "resonance PV {pv} vs analytic A*g*Q/wn {expected} (rel {rel})"
    );
}

/// PVSS / srs_analyze consistency: pseudo-acceleration PA = wn * PV approaches
/// the maximax-acceleration SRS (in consistent units) at high frequency, where
/// the spring term dominates the damping term. Equivalently the plateau
/// relationship PV ~ (srs_analyze_in_g * G_TO_IN_S2) / wn holds to a documented
/// tolerance across the band, tightening toward 1.0 at high frequency.
#[test]
fn srs_pseudo_velocity_matches_srs_analyze_plateau() {
    let fs = 50_000.0;
    let amp = 10.0;
    let tau = 0.005;
    let pulse = half_sine_pulse(amp, tau, fs, 0.5);
    let q = 10.0;

    let freqs = srs::octave_freqs(10.0, 1000.0, OctaveFraction::Third);
    let sa = srs::srs_analyze(&pulse, fs, &freqs, q);
    let pv = srs::srs_pseudo_velocity(&pulse, fs, &freqs, q);

    // Across the whole band PV ~ SA*g/wn within 1% (the residual is the
    // pseudo-acceleration vs maximax-absolute-acceleration damping term at Q=10).
    for (i, &f) in freqs.iter().enumerate() {
        let wn = 2.0 * PI * f;
        let sa_as_pv = sa[i] * G_TO_IN_S2 / wn;
        let rel = (pv[i] - sa_as_pv).abs() / sa_as_pv;
        assert!(
            rel < 0.01,
            "f={f}: PV {} vs SA*g/wn {sa_as_pv} (rel {rel})",
            pv[i]
        );
    }
    // The relationship tightens at high frequency (spring term dominates): the
    // top band should agree to < 0.1%.
    let top = freqs.len() - 1;
    let wn = 2.0 * PI * freqs[top];
    let sa_as_pv = sa[top] * G_TO_IN_S2 / wn;
    assert!(
        (pv[top] - sa_as_pv).abs() / sa_as_pv < 1e-3,
        "high-f PV/SA plateau should be tight"
    );
}

/// Low-frequency PVSS asymptote at high Q approaches the pulse velocity change
/// dV = G_TO_IN_S2 * A * 2T/pi (undamped velocity regime).
#[test]
fn srs_pseudo_velocity_low_freq_asymptote_is_velocity_change() {
    let fs = 50_000.0;
    let amp = 10.0;
    let tau = 0.005;
    let pulse = half_sine_pulse(amp, tau, fs, 2.0); // long tail for slow residual
    let q = 100.0; // light damping -> close to undamped asymptote

    let pv = srs::srs_pseudo_velocity(&pulse, fs, &[2.0], q)[0];
    let dv = G_TO_IN_S2 * amp * 2.0 * tau / PI;
    let rel = (pv - dv).abs() / dv;
    assert!(
        rel < 0.05,
        "low-f PVSS {pv} vs velocity change dV {dv} (rel {rel})"
    );
}

/// PVSS edge cases mirror srs_analyze: empty record -> zeros; f_n <= 0 (rigid) ->
/// 0; f_n >= Nyquist -> 0.
#[test]
fn srs_pseudo_velocity_edge_cases() {
    let empty = srs::srs_pseudo_velocity(&[], 1000.0, &[100.0, 200.0], 10.0);
    assert_eq!(empty, vec![0.0, 0.0]);

    let sig = half_sine_pulse(5.0, 0.005, 10_000.0, 0.05);
    let fs = 10_000.0;
    let out = srs::srs_pseudo_velocity(&sig, fs, &[0.0, -5.0, fs / 2.0, fs], 10.0);
    assert_eq!(out[0], 0.0, "rigid body f_n=0 -> PV 0");
    assert_eq!(out[1], 0.0, "negative f_n -> PV 0");
    assert_eq!(out[2], 0.0, "at Nyquist -> PV 0");
    assert_eq!(out[3], 0.0, "above Nyquist -> PV 0");
}

// ---------------------------------------------------------------------------
// classical pulse generators
// ---------------------------------------------------------------------------

/// Every generator's sample count is round(fs*duration_ms/1000), matching the
/// delegated backend's half-sine reference convention.
#[test]
fn generators_sample_count_convention() {
    let fs = 5120.0;
    let dur = 11.0; // ms -> 5120*11/1000 = 56.32 -> 56 samples
    let expect = pulse::sample_count(dur, fs);
    assert_eq!(expect, 56);
    assert_eq!(pulse::half_sine(20.0, dur, fs).len(), expect);
    assert_eq!(pulse::haversine(20.0, dur, fs).len(), expect);
    assert_eq!(pulse::terminal_peak_sawtooth(20.0, dur, fs).len(), expect);
    assert_eq!(pulse::initial_peak_sawtooth(20.0, dur, fs).len(), expect);
    assert_eq!(pulse::trapezoid(20.0, dur, fs, 0.2, 0.2).len(), expect);
    assert_eq!(pulse::rectangle(20.0, dur, fs, 0.1, 0.1).len(), expect);
}

/// half_sine: endpoints ~0, peak == amplitude exactly for odd N (matches the
/// delegated backend's reference convention).
#[test]
fn generator_half_sine_peak_and_endpoints() {
    // choose a duration that yields odd N=57 at 5120 Hz.
    let fs = 5120.0;
    let dur = 57.0 / fs * 1000.0;
    let w = pulse::half_sine(20.0, dur, fs);
    assert_eq!(w.len(), 57);
    assert_eq!(w[0], 0.0);
    assert!(w.last().unwrap().abs() < 1e-9);
    let peak = w.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!(
        (peak - 20.0).abs() < 1e-12,
        "odd-N half-sine peak {peak} != 20"
    );
    // never exceeds amplitude
    assert!(w.iter().all(|&v| v <= 20.0 + 1e-9));
}

/// haversine: endpoints exactly 0, peak == amplitude at odd-N midpoint, and both
/// endpoint SLOPES are ~0 (the smoother-than-half-sine property).
#[test]
fn generator_haversine_peak_and_zero_slope_ends() {
    let fs = 5120.0;
    let dur = 57.0 / fs * 1000.0; // odd N=57
    let w = pulse::haversine(15.0, dur, fs);
    assert_eq!(w.len(), 57);
    assert!(w[0].abs() < 1e-12);
    assert!(w.last().unwrap().abs() < 1e-9);
    let peak = w.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!((peak - 15.0).abs() < 1e-9, "haversine peak {peak} != 15");
    // zero slope at the start: first step is tiny compared to a half-sine's.
    let first_step = (w[1] - w[0]).abs();
    let hs = pulse::half_sine(15.0, dur, fs);
    let hs_first_step = (hs[1] - hs[0]).abs();
    assert!(
        first_step < hs_first_step,
        "haversine onset slope {first_step} should be gentler than half-sine {hs_first_step}"
    );
}

/// terminal_peak_sawtooth: monotonically increasing, peak == amplitude at the
/// LAST sample, starts at 0.
#[test]
fn generator_terminal_peak_sawtooth_monotone() {
    let w = pulse::terminal_peak_sawtooth(30.0, 10.0, 10_000.0);
    assert_eq!(w[0], 0.0);
    assert!((w.last().unwrap() - 30.0).abs() < 1e-12);
    for pair in w.windows(2) {
        assert!(pair[1] >= pair[0], "sawtooth not monotone increasing");
    }
}

/// initial_peak_sawtooth: monotonically decreasing, peak == amplitude at the
/// FIRST sample, ends at 0. It is the time-reverse of the terminal-peak.
#[test]
fn generator_initial_peak_sawtooth_monotone() {
    let w = pulse::initial_peak_sawtooth(30.0, 10.0, 10_000.0);
    assert!((w[0] - 30.0).abs() < 1e-12);
    assert!(w.last().unwrap().abs() < 1e-12);
    for pair in w.windows(2) {
        assert!(pair[1] <= pair[0], "sawtooth not monotone decreasing");
    }
    // time-reverse of terminal-peak.
    let t = pulse::terminal_peak_sawtooth(30.0, 10.0, 10_000.0);
    for (a, b) in w.iter().zip(t.iter().rev()) {
        assert!(
            (a - b).abs() < 1e-12,
            "initial-peak != reversed terminal-peak"
        );
    }
}

/// trapezoid: the plateau occupies (1 - rise - fall) of the samples at exactly
/// the amplitude, the rise is monotone up, the fall is monotone down, and
/// endpoints are 0.
#[test]
fn generator_trapezoid_plateau_fraction() {
    let fs = 10_000.0;
    let dur = 10.0; // ms -> 100 samples
    let amp = 25.0;
    let (rf, ff) = (0.25, 0.25);
    let w = pulse::trapezoid(amp, dur, fs, rf, ff);
    let n = w.len();
    assert_eq!(n, 100);
    assert_eq!(w[0], 0.0);
    assert!(w.last().unwrap().abs() < 1e-9);

    // count plateau samples (at the amplitude to float tolerance).
    let plateau: usize = w.iter().filter(|&&v| (v - amp).abs() < 1e-9).count();
    // expected plateau fraction ~ 1 - rf - ff = 0.5 of the samples; allow a few
    // samples of slack for the discrete rise/fall boundary rounding.
    let expected = ((1.0 - rf - ff) * (n - 1) as f64).round() as usize;
    assert!(
        (plateau as i64 - expected as i64).abs() <= 2,
        "plateau count {plateau} vs expected ~{expected}"
    );
    // peak never exceeds amplitude, and the plateau reaches it.
    let peak = w.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!((peak - amp).abs() < 1e-9, "trapezoid peak {peak} != {amp}");

    // rise monotone up to the plateau, fall monotone down after.
    let rise_end = (rf * (n - 1) as f64).floor() as usize;
    for i in 1..=rise_end {
        assert!(w[i] >= w[i - 1] - 1e-12, "rise not monotone at {i}");
    }
    let fall_start = ((1.0 - ff) * (n - 1) as f64).ceil() as usize;
    for i in (fall_start + 1)..n {
        assert!(w[i] <= w[i - 1] + 1e-12, "fall not monotone at {i}");
    }
}

/// A trapezoid whose rise+fall exceed 1 is scaled to a triangle (zero plateau),
/// never inverted. The vertex reaches the amplitude when it lands on a sample;
/// otherwise (even/odd grid effect, same caveat as the half-sine midpoint) the
/// peak sample is within one bin. Choose a grid where the 50/50 vertex lands on a
/// sample (odd N) to check the exact case, and a general grid for the bound.
#[test]
fn generator_trapezoid_oversized_edges_degenerate_to_triangle() {
    // Odd N=101 so the normalized vertex 0.5 = 50/(101-1) falls exactly on sample 50.
    let fs = 10_000.0;
    let dur = 10.1; // ms -> round(10000*10.1/1000) = 101 samples
    let w = pulse::trapezoid(10.0, dur, fs, 0.8, 0.8);
    assert_eq!(w.len(), 101);
    let peak = w.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!(
        (peak - 10.0).abs() < 1e-9,
        "triangle peak {peak} != 10 on a grid where the vertex lands on a sample"
    );
    // never inverted, never exceeds the amplitude, on any grid.
    let w2 = pulse::trapezoid(10.0, 10.0, 10_000.0, 0.8, 0.8);
    assert!(w2.iter().all(|&v| (-1e-12..=10.0 + 1e-9).contains(&v)));
    // even-N vertex is within one bin of the amplitude (documented caveat).
    let near = w2.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!(
        (near - 10.0).abs() / 10.0 < 0.02,
        "even-N vertex {near} within a bin"
    );
}

/// rectangle with finite edges: reduces to a trapezoid (bounded slew); with zero
/// edges it is a flat block at the amplitude (documented sharp-edge case).
#[test]
fn generator_rectangle_edges_and_flat_block() {
    let fs = 10_000.0;
    let dur = 10.0;
    let amp = 12.0;
    // finite edges -> equals the trapezoid.
    let r = pulse::rectangle(amp, dur, fs, 0.1, 0.1);
    let t = pulse::trapezoid(amp, dur, fs, 0.1, 0.1);
    assert_eq!(r, t);
    // most of the pulse is at the amplitude (flat top).
    let plateau = r.iter().filter(|&&v| (v - amp).abs() < 1e-9).count();
    assert!(plateau > r.len() / 2, "rectangle should be mostly flat top");
    // zero edges -> flat block, every sample at amplitude.
    let block = pulse::rectangle(amp, dur, fs, 0.0, 0.0);
    assert!(block.iter().all(|&v| (v - amp).abs() < 1e-12));
}

/// Generators produce empty vectors for degenerate (non-positive) requests
/// without panicking.
#[test]
fn generators_degenerate_requests() {
    assert!(pulse::half_sine(20.0, 0.0, 5120.0).is_empty());
    assert!(pulse::haversine(20.0, 11.0, 0.0).is_empty());
    assert!(pulse::terminal_peak_sawtooth(20.0, -1.0, 5120.0).is_empty());
    // Single-sample degenerate: 5120*0.05/1000 = 0.256 rounds to 0, but with a
    // positive duration sample_count clamps to >= 1, so one sample at the
    // amplitude is returned (the HalfSinePulse degenerate-single-sample rule).
    let one = pulse::half_sine(7.0, 0.05, 5120.0);
    assert_eq!(one, vec![7.0]);
}

/// A generated half-sine, fed through pulse_kinematics, reproduces the analytic
/// velocity change - ties the generator and the kinematics primitive together.
#[test]
fn generated_half_sine_kinematics_close_the_loop() {
    let fs = 50_000.0;
    let amp = 20.0; // g
    let dur_ms = 11.0; // -> tau = 0.011 s
    let mut w = pulse::half_sine(amp, dur_ms, fs);
    w.extend(std::iter::repeat_n(0.0, (0.05 * fs) as usize));
    let k = shock::pulse_kinematics(&w, fs);

    let tau = dur_ms / 1000.0;
    let dv = G_TO_IN_S2 * amp * 2.0 * tau / PI;
    let rel = (k.residual_velocity_ips - dv).abs() / dv;
    assert!(
        rel < 0.01,
        "generated half-sine residual velocity {} vs analytic dV {dv} (rel {rel})",
        k.residual_velocity_ips
    );
    assert!((k.peak_accel_g - amp).abs() / amp < 1e-3);
}
