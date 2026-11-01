//! Golden-vector tests for the Sine additive eigen-dsp primitives:
//! phase-continuous sweep NCO + sweep laws (`sweep`), bandwidth modes /
//! adaptive tracking / THD bank (`tracking`), resonance primitives
//! (`resonance`), and the additive `TrackingFilter::set_bandwidth` +
//! `AmplitudeServo::hold_tick` methods.
//!
//! Convention (per tests/golden.rs): every reference is ANALYTIC
//! (closed-form), never a value copied out of the code under test.

use eigen_dsp::resonance::{half_power, PhaseTracker, PhaseTrackerConfig};
use eigen_dsp::sine::{AmplitudeServo, TrackingFilter};
use eigen_dsp::sweep::{octaves_between, sweep_seconds, SweepOscillator, SweepRate};
use eigen_dsp::tracking::{AdaptiveTrackingFilter, BandwidthMode, ThdBank};
use std::f64::consts::TAU;

// ---------------------------------------------------------------------------
// sweep: NCO phase continuity + sweep laws
// ---------------------------------------------------------------------------

/// Phase continuity through a rate change AND a direction reversal (both are
/// frequency-DERIVATIVE discontinuities): the per-sample phase increment must
/// never exceed the largest commanded `2*pi*f/fs`, i.e. no phase jump ever.
#[test]
fn sweep_oscillator_phase_continuous_across_rate_change_and_reversal() {
    let fs = 8192.0;
    let mut osc = SweepOscillator::new(fs, 100.0);
    let mut phases: Vec<f64> = Vec::new();
    let mut f_max = 0.0f64;

    // Segment A: sweep up at 60 oct/min from 100 Hz for 1 s (100 -> 200 Hz).
    // Segment B: REVERSE at -120 oct/min from 200 Hz for 0.5 s (200 -> 100 Hz).
    // Segment C: linear 800 Hz/s from 100 Hz for 0.5 s (100 -> 500 Hz).
    let segments: [(SweepRate, f64, f64); 3] = [
        (SweepRate::LogOctPerMin(60.0), 100.0, 1.0),
        (SweepRate::LogOctPerMin(-120.0), 200.0, 0.5),
        (SweepRate::LinearHzPerSec(800.0), 100.0, 0.5),
    ];
    for (rate, f_seg0, dur_s) in segments {
        let n = (dur_s * fs) as usize;
        for k in 0..n {
            let t = k as f64 / fs;
            let f_cmd = rate.freq_at(f_seg0, t);
            f_max = f_max.max(f_cmd);
            osc.set_frequency(f_cmd);
            let (_s, _f, theta) = osc.next();
            phases.push(theta);
        }
    }

    let max_inc = TAU * f_max / fs + 1e-9;
    for (i, pair) in phases.windows(2).enumerate() {
        // wrapped forward increment in [0, 2*pi)
        let inc = (pair[1] - pair[0]).rem_euclid(TAU);
        assert!(
            inc <= max_inc,
            "phase jump {inc} rad at sample {i} exceeds max increment {max_inc}"
        );
    }
}

/// Log sweep law: after t seconds at R oct/min the frequency is
/// `f0 * 2^(R*t/60)` (checked via powf, octave count, and composition).
#[test]
fn sweep_log_law_matches_analytic() {
    let f0 = 50.0;
    let r = 6.0; // oct/min
    let law = SweepRate::LogOctPerMin(r);
    for &t in &[0.0, 1.0, 7.5, 30.0, 60.0, 123.4] {
        let f = law.freq_at(f0, t);
        let expected = f0 * 2.0f64.powf(r * t / 60.0);
        assert!(
            (f - expected).abs() / expected < 1e-9,
            "log law at t={t}: {f} vs {expected}"
        );
        // octave count consistency
        assert!(
            (octaves_between(f0, f) - r * t / 60.0).abs() < 1e-9,
            "octave count at t={t}"
        );
    }
    // composition: sweeping t1 then t2 == sweeping t1 + t2
    let f_mid = law.freq_at(f0, 12.3);
    let f_comp = law.freq_at(f_mid, 7.7);
    let f_direct = law.freq_at(f0, 20.0);
    assert!((f_comp - f_direct).abs() / f_direct < 1e-12);

    // one octave at 6 oct/min takes 10 s (both directions)
    assert!((sweep_seconds(50.0, 100.0, law) - 10.0).abs() < 1e-9);
    assert!((sweep_seconds(100.0, 50.0, law) - 10.0).abs() < 1e-9);
    // zero rate never gets there
    assert!(sweep_seconds(50.0, 100.0, SweepRate::LogOctPerMin(0.0)).is_infinite());

    // downward law
    let down = SweepRate::LogOctPerMin(-6.0);
    let f = down.freq_at(100.0, 10.0);
    assert!((f - 50.0).abs() / 50.0 < 1e-9, "down-sweep after 10 s: {f}");
}

/// Linear sweep law: `f(t) = f0 + rate * t`.
#[test]
fn sweep_linear_law_matches_analytic() {
    let law = SweepRate::LinearHzPerSec(2.5);
    assert!((law.freq_at(20.0, 8.0) - 40.0).abs() < 1e-12);
    assert!((law.freq_at(20.0, 0.0) - 20.0).abs() < 1e-12);
    assert!((sweep_seconds(20.0, 40.0, law) - 8.0).abs() < 1e-12);
    assert!((sweep_seconds(40.0, 20.0, law) - 8.0).abs() < 1e-12);
    assert!(sweep_seconds(20.0, 40.0, SweepRate::LinearHzPerSec(0.0)).is_infinite());
    let down = SweepRate::LinearHzPerSec(-4.0);
    assert!((down.freq_at(100.0, 5.0) - 80.0).abs() < 1e-12);
}

/// At a fixed frequency the NCO reproduces the analytic sine to float
/// precision over tens of thousands of samples (wrap correctness + no
/// accumulated drift beyond rounding).
#[test]
fn sweep_oscillator_matches_analytic_sine_at_fixed_freq() {
    let fs = 8192.0;
    let f = 441.0;
    let n = 40_960usize;
    let mut osc = SweepOscillator::new(fs, f);
    let delta = TAU * f / fs;
    let mut worst = 0.0f64;
    for k in 0..n {
        let (s, _f, _theta) = osc.next();
        let reference = (delta * k as f64).rem_euclid(TAU).sin();
        worst = worst.max((s - reference).abs());
    }
    assert!(worst < 1e-8, "NCO worst abs error {worst}");
}

// ---------------------------------------------------------------------------
// tracking: bandwidth modes, adaptive filter, THD bank
// ---------------------------------------------------------------------------

/// Bandwidth-mode arithmetic including the image-rejection clamp `<= 0.5*f`
/// in BOTH modes, and the proportional cap.
#[test]
fn bandwidth_mode_effective_bw_and_clamp() {
    let fixed = BandwidthMode::FixedHz(10.0);
    assert!((fixed.effective_bw(200.0) - 10.0).abs() < 1e-12);
    // low drive frequency: clamped to 0.5*f, not the fixed value
    assert!((fixed.effective_bw(8.0) - 4.0).abs() < 1e-12);

    let prop = BandwidthMode::Proportional {
        pct: 10.0,
        max_hz: 50.0,
    };
    assert!((prop.effective_bw(100.0) - 10.0).abs() < 1e-12);
    // cap wins at high f
    assert!((prop.effective_bw(1000.0) - 50.0).abs() < 1e-12);
    // proportional rule below the clamp at low f
    assert!((prop.effective_bw(4.0) - 0.4).abs() < 1e-12);
    // aggressive percentage still clamped to 0.5*f
    let wide = BandwidthMode::Proportional {
        pct: 200.0,
        max_hz: 1e9,
    };
    assert!((wide.effective_bw(10.0) - 5.0).abs() < 1e-12);

    // settling budget is 1/bw
    let filt = AdaptiveTrackingFilter::new(8192.0, BandwidthMode::FixedHz(10.0));
    assert!((filt.settling_s(200.0) - 0.1).abs() < 1e-12);
    assert!((filt.settling_s(8.0) - 0.25).abs() < 1e-12);
}

/// Steady tone through the adaptive filter: amplitude within 0.5% of the true
/// peak amplitude after settling.
#[test]
fn adaptive_tracking_steady_tone_amplitude_within_half_percent() {
    let fs = 8192.0;
    let f0 = 100.0;
    let a = 2.5;
    let mut osc = SweepOscillator::new(fs, f0);
    let mut filt = AdaptiveTrackingFilter::new(
        fs,
        BandwidthMode::Proportional {
            pct: 5.0,
            max_hz: 20.0,
        },
    );
    let n = (3.0 * fs) as usize;
    let settle = fs as usize; // 1 s >> settling_s(100 Hz) = 0.2 s
    for k in 0..n {
        let (s, f, _theta) = osc.next();
        let (amp, _phase) = filt.update(a * s, f);
        if k >= settle {
            assert!(
                (amp - a).abs() / a < 0.005,
                "steady amplitude {amp} vs {a} at sample {k}"
            );
        }
    }
}

/// Swept tone at 1 oct/min: constant-amplitude readout stays within 1%.
#[test]
fn adaptive_tracking_swept_tone_amplitude_within_one_percent() {
    let fs = 8192.0;
    let a = 1.7;
    let law = SweepRate::LogOctPerMin(1.0);
    let mut osc = SweepOscillator::new(fs, 100.0);
    let mut filt = AdaptiveTrackingFilter::new(
        fs,
        BandwidthMode::Proportional {
            pct: 5.0,
            max_hz: 20.0,
        },
    );
    let n = (10.0 * fs) as usize;
    let settle = (2.0 * fs) as usize;
    for k in 0..n {
        let t = k as f64 / fs;
        osc.set_frequency(law.freq_at(100.0, t));
        let (s, f, _theta) = osc.next();
        let (amp, _phase) = filt.update(a * s, f);
        if k >= settle {
            assert!(
                (amp - a).abs() / a < 0.01,
                "swept amplitude {amp} vs {a} at sample {k}"
            );
        }
    }
}

/// THD golden: 1.0*sin(t) + 0.05*sin(2t) + 0.03*sin(3t) has
/// THD = sqrt(0.05^2 + 0.03^2)/1.0 = 0.058310 (analytic), within 2%.
#[test]
fn thd_bank_known_harmonic_mix() {
    let fs = 8192.0;
    let f0 = 100.0;
    let mut osc = SweepOscillator::new(fs, f0);
    // Narrow lock-in corner (1 Hz) so fundamental leakage through the
    // two-pole LP stays far below the harmonic amplitudes under test.
    let mut bank = ThdBank::new(fs, 1.0);
    let n = (5.0 * fs) as usize;
    for _ in 0..n {
        let (_s, f, theta) = osc.next();
        let x = theta.sin() + 0.05 * (2.0 * theta).sin() + 0.03 * (3.0 * theta).sin();
        bank.update(x, f, theta);
    }
    let expected = (0.05f64 * 0.05 + 0.03 * 0.03).sqrt();
    let thd = bank.thd(1.0);
    assert!(
        (thd - expected).abs() / expected < 0.02,
        "THD {thd} vs analytic {expected}"
    );
    let amps = bank.harmonic_amplitudes();
    assert!((amps[0] - 0.05).abs() / 0.05 < 0.02, "A2 = {}", amps[0]);
    assert!((amps[1] - 0.03).abs() / 0.03 < 0.02, "A3 = {}", amps[1]);
    for (i, &ak) in amps.iter().enumerate().skip(2) {
        assert!(ak < 0.005, "A{} = {ak} should be ~0", i + 2);
    }
    // THD is undefined without a fundamental
    assert!(bank.thd(0.0).is_nan());
    assert!(bank.thd(-1.0).is_nan());
}

/// Harmonics at or above Nyquist are skipped (and hold no stale amplitude):
/// a strong 9th harmonic above fs/2 must not appear in the THD.
#[test]
fn thd_bank_skips_harmonics_at_or_above_nyquist() {
    let fs = 8192.0; // Nyquist 4096 Hz
    let f0 = 500.0; // 9*f0 = 4500 >= 4096 -> skipped
    let mut osc = SweepOscillator::new(fs, f0);
    let mut bank = ThdBank::new(fs, 1.0);
    let n = (4.0 * fs) as usize;
    for _ in 0..n {
        let (_s, f, theta) = osc.next();
        let x = theta.sin() + 0.5 * (9.0 * theta).sin();
        bank.update(x, f, theta);
    }
    let thd = bank.thd(1.0);
    assert!(
        thd < 0.01,
        "THD {thd} should exclude the out-of-band 9th harmonic"
    );
}

// ---------------------------------------------------------------------------
// resonance: half-power Q + phase tracker
// ---------------------------------------------------------------------------

/// SDOF resonator magnitude, analytic: `1/sqrt((1-r^2)^2 + (2*zeta*r)^2)`.
fn sdof_mag(f: f64, f_n: f64, q: f64) -> f64 {
    let zeta = 1.0 / (2.0 * q);
    let r = f / f_n;
    1.0 / ((1.0 - r * r).powi(2) + (2.0 * zeta * r).powi(2)).sqrt()
}

/// Base-excited SDOF transmissibility phase, analytic closed form:
/// `H(r) = (1 + i*2*zeta*r)/(1 - r^2 + i*2*zeta*r)`, so
/// `phi = atan2(2*zeta*r, 1) - atan2(2*zeta*r, 1 - r^2)`.
/// Under this convention phi starts near 0 below resonance, passes
/// `atan(2*zeta) - pi/2 ~ -pi/2` at `f_n`, with slope `~ -2Q/f_n` rad/Hz there.
fn transmissibility_phase(f: f64, f_n: f64, q: f64) -> f64 {
    let zeta = 1.0 / (2.0 * q);
    let r = f / f_n;
    (2.0 * zeta * r).atan2(1.0) - (2.0 * zeta * r).atan2(1.0 - r * r)
}

/// Half-power fit on an analytic SDOF magnitude curve recovers Q, the peak
/// frequency, and the peak amplitude (references: Q_est -> Q with the -3 dB
/// bandwidth `~ f_n/Q`; peak amplitude `~ Q` for large Q).
#[test]
fn half_power_recovers_sdof_q() {
    let f_n = 250.0;
    let q = 50.0;
    let freqs: Vec<f64> = (0..=2000).map(|i| 200.0 + 0.05 * i as f64).collect();
    let amps: Vec<f64> = freqs.iter().map(|&f| sdof_mag(f, f_n, q)).collect();
    let est = half_power(&freqs, &amps).expect("well-posed peak must fit");
    assert!(
        (est.q - q).abs() / q < 0.02,
        "Q estimate {} vs analytic {q}",
        est.q
    );
    // magnitude peak sits at f_n*sqrt(1 - 2*zeta^2), within a grid step or two
    let zeta = 1.0 / (2.0 * q);
    let f_peak_true = f_n * (1.0 - 2.0 * zeta * zeta).sqrt();
    assert!(
        (est.f_peak - f_peak_true).abs() < 0.15,
        "f_peak {} vs analytic {f_peak_true}",
        est.f_peak
    );
    // peak amplitude ~ Q (exact: Q/sqrt(1 - zeta^2/... ) ~ 50.0025)
    assert!(
        (est.amp_peak - q).abs() / q < 0.01,
        "amp_peak {} vs ~{q}",
        est.amp_peak
    );
}

/// Ill-posed inputs return None instead of a bogus estimate.
#[test]
fn half_power_rejects_ill_posed_inputs() {
    // peak at the edge of the span
    let freqs = [1.0, 2.0, 3.0, 4.0];
    let rising = [1.0, 2.0, 3.0, 4.0];
    assert!(half_power(&freqs, &rising).is_none());
    // mismatched lengths
    assert!(half_power(&freqs, &[1.0, 2.0]).is_none());
    // too short
    assert!(half_power(&[1.0, 2.0], &[1.0, 2.0]).is_none());
    // no positive peak
    assert!(half_power(&freqs, &[0.0, 0.0, 0.0, 0.0]).is_none());
    // peak that never falls to half power inside the span
    let flat = [10.0, 10.5, 11.0, 10.5, 10.0];
    let f5 = [1.0, 2.0, 3.0, 4.0, 5.0];
    assert!(half_power(&f5, &flat).is_none());
}

/// The phase tracker converges from 5% off onto f_n and holds within 0.1%.
/// Reference plant: the analytic transmissibility phase above.
#[test]
fn phase_tracker_converges_onto_resonance_and_holds() {
    let f_n = 250.0;
    let q = 25.0;
    let cfg = PhaseTrackerConfig {
        phase_setpoint_rad: transmissibility_phase(f_n, f_n, q),
        slope_rad_per_hz: -2.0 * q / f_n, // analytic SDOF slope at resonance
        gain: 0.5,
        band: (200.0, 300.0),
        max_step_hz: 2.0,
        exit_ticks: 5,
    };
    let mut tr = PhaseTracker::new(cfg, f_n * 1.05); // start 5% high
    for _ in 0..200 {
        let phi = transmissibility_phase(tr.freq_hz(), f_n, q);
        tr.update(phi);
    }
    assert!(
        (tr.freq_hz() - f_n).abs() / f_n < 0.001,
        "converged to {} vs {f_n}",
        tr.freq_hz()
    );
    // and HOLDS: 50 more updates stay within 0.1%
    for _ in 0..50 {
        let phi = transmissibility_phase(tr.freq_hz(), f_n, q);
        tr.update(phi);
        assert!(
            (tr.freq_hz() - f_n).abs() / f_n < 0.001,
            "hold drifted to {}",
            tr.freq_hz()
        );
    }
    assert!(!tr.band_exit());
}

/// DRIFT: f_n ramps 250 -> 235 Hz; the tracker follows within 0.5 Hz.
#[test]
fn phase_tracker_follows_drifting_resonance() {
    let q = 25.0;
    let f_n0 = 250.0;
    let cfg = PhaseTrackerConfig {
        phase_setpoint_rad: transmissibility_phase(f_n0, f_n0, q),
        slope_rad_per_hz: -2.0 * q / f_n0,
        gain: 0.5,
        band: (200.0, 300.0),
        max_step_hz: 2.0,
        exit_ticks: 5,
    };
    let mut tr = PhaseTracker::new(cfg, f_n0);
    // pre-converge on the stationary resonance
    for _ in 0..100 {
        let phi = transmissibility_phase(tr.freq_hz(), f_n0, q);
        tr.update(phi);
    }
    // ramp f_n down 15 Hz over 300 ticks (0.05 Hz per update)
    let n = 300;
    for i in 0..n {
        let f_n = f_n0 - 15.0 * (i + 1) as f64 / n as f64;
        let phi = transmissibility_phase(tr.freq_hz(), f_n, q);
        tr.update(phi);
        if i >= 20 {
            assert!(
                (tr.freq_hz() - f_n).abs() < 0.5,
                "tracker {} vs drifting f_n {f_n} at tick {i}",
                tr.freq_hz()
            );
        }
    }
    assert!(!tr.band_exit());
}

/// The resonance walks below the band edge: the commanded frequency clamps at
/// the edge and band_exit latches after exit_ticks consecutive pinned updates
/// (and STAYS latched after the resonance returns).
#[test]
fn phase_tracker_band_exit_latches_at_edge() {
    let q = 25.0;
    let f_n0 = 250.0;
    let band = (240.0, 260.0);
    let cfg = PhaseTrackerConfig {
        phase_setpoint_rad: transmissibility_phase(f_n0, f_n0, q),
        slope_rad_per_hz: -2.0 * q / f_n0,
        gain: 0.5,
        band,
        max_step_hz: 2.0,
        exit_ticks: 5,
    };
    let mut tr = PhaseTracker::new(cfg, f_n0);
    // resonance far below the band: phase error keeps pushing down
    let f_n = 230.0;
    for _ in 0..40 {
        let phi = transmissibility_phase(tr.freq_hz(), f_n, q);
        tr.update(phi);
    }
    assert!(tr.band_exit(), "band_exit must latch at the low edge");
    assert!(
        (tr.freq_hz() - band.0).abs() < 1e-12,
        "frequency must stay clamped at the band edge, got {}",
        tr.freq_hz()
    );
    // latch survives the resonance coming back in band
    for _ in 0..20 {
        let phi = transmissibility_phase(tr.freq_hz(), 250.0, q);
        tr.update(phi);
    }
    assert!(tr.band_exit(), "band_exit latch must persist until reset");
    assert!(tr.freq_hz() > band.0, "tracking resumes inside the band");
    // reset clears the latch
    tr.reset(250.0);
    assert!(!tr.band_exit());
}

// ---------------------------------------------------------------------------
// additive sine methods: set_bandwidth, hold_tick
// ---------------------------------------------------------------------------

/// Retuning the tracking-filter bandwidth mid-tone must not glitch the
/// amplitude readout by more than 1% (state-preserving retune).
#[test]
fn tracking_filter_set_bandwidth_mid_tone_no_glitch() {
    let fs = 8192.0;
    let f = 200.0;
    let a = 1.3;
    let mut tf = TrackingFilter::new(fs, 5.0);
    let n_settle = (3.0 * fs) as usize;
    for k in 0..n_settle {
        let x = a * (TAU * f * k as f64 / fs).sin();
        tf.update(x, f);
    }
    // sanity: settled before the retune
    let x = a * (TAU * f * n_settle as f64 / fs).sin();
    let (amp0, _) = tf.update(x, f);
    assert!((amp0 - a).abs() / a < 0.01);

    tf.set_bandwidth(20.0);
    let n_after = (2.0 * fs) as usize;
    for k in (n_settle + 1)..(n_settle + 1 + n_after) {
        let x = a * (TAU * f * k as f64 / fs).sin();
        let (amp, _) = tf.update(x, f);
        assert!(
            (amp - a).abs() / a < 0.01,
            "amplitude glitch {amp} vs {a} at sample {k} after retune"
        );
    }
}

/// hold_tick freezes the drive under a sustained error (no windup) and the
/// first update after release has no proportional/derivative kick: its step
/// is bounded by the integral term ki*e*dt alone.
#[test]
fn amplitude_servo_hold_tick_no_windup_no_resume_kick() {
    let (kp, ki) = (0.3, 2.0);
    let dt = 0.01;
    let mut servo = AmplitudeServo::new(1.0, 20.0).with_gains(kp, ki);
    // converge against an instantaneous-gain plant (g = 2 -> drive 0.5)
    let g = 2.0;
    let mut drive = servo.drive_scale();
    for _ in 0..3000 {
        drive = servo.update(g * drive, dt);
    }
    assert!((g * drive - 1.0).abs() < 1e-6, "loop must be converged");

    // 10 s of sustained -6 dB error while an external governor binds: frozen
    let drive_before = servo.drive_scale();
    for _ in 0..1000 {
        let d = servo.hold_tick(0.5);
        assert_eq!(
            d, drive_before,
            "hold_tick must not move the drive (no windup)"
        );
    }
    assert_eq!(servo.drive_scale(), drive_before);

    // resume: step bounded by kp*0 + ki*e*dt (no derivative kick)
    let d1 = servo.update(0.5, dt);
    let step_db = 20.0 * (d1 / drive_before).log10();
    let e_db = 20.0 * (1.0f64 / 0.5).log10(); // +6.0206 dB
    let expected = ki * e_db * dt; // 0.1204 dB, under the 0.2 dB rate cap
    assert!(
        (step_db - expected).abs() < 1e-9,
        "resume step {step_db} dB vs integral-only {expected} dB"
    );
}
