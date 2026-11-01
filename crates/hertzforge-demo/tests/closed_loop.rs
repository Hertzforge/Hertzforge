//! End-to-end, HARDWARE-FREE closed-loop convergence test.
//!
//! Runs the Random-on-Sim control loop headless through the real [`hertzforge_engine::Engine`]
//! and asserts that it CONVERGES: the measured control-channel PSD tracks the target
//! across the interior band after settling, the drive never runs away, and no healthy-run
//! fault (servo dropout / ceiling, or an engine abort) ever trips.
//!
//! This is the payoff test for the first end-to-end run of Layers 1-3: the control DSP
//! (COLA / FRF / servo), the engine (ring / timebase / watchdog / output contract /
//! capture), and the sim plant, tied together with no hardware.

use hertzforge_demo::{run_demo, DemoConfig, Phase};

/// The loop converges the in-band response PSD to within ~3 dB of the target after
/// settling, and does so cleanly (no runaway, no spurious fault).
#[test]
fn random_on_sim_closed_loop_converges() {
    let cfg = DemoConfig::default();
    let target_rms = cfg.target_inband_rms();
    let result = run_demo(cfg, 60);

    // The whole loop ran through the real engine: frames flowed and were captured.
    assert!(
        result.total_frames > 0 && result.total_captured > 0,
        "no frames flowed through the engine/capture"
    );
    // Every frame the engine processed was captured through the real CaptureSink seam.
    assert_eq!(
        result.total_frames, result.total_captured,
        "engine dropped frames from the capture path"
    );

    // No segment aborted for any reason other than ending cleanly.
    assert!(
        result.abnormal_stop.is_none(),
        "a segment aborted abnormally: {:?}",
        result.abnormal_stop
    );

    // The loop reached the full-level HOLD phase and stayed there.
    assert_eq!(
        result.final_phase,
        Phase::Hold,
        "loop did not reach the HOLD phase"
    );
    assert!(
        result.hold_windows >= 10,
        "too few hold windows to call it settled: {}",
        result.hold_windows
    );

    // --- Convergence: measured in-band PSD within ~3 dB of target after settling. ---
    let settled_worst = result
        .settled_worst_err_db(10)
        .expect("no valid error measurements");
    let settled_mean = result.settled_mean_err_db(10).unwrap();
    assert!(
        settled_worst <= 3.0,
        "in-band PSD did not converge to within 3 dB after settling \
         (worst {settled_worst:.2} dB, mean {settled_mean:.2} dB)"
    );

    // --- No runaway: the drive stays bounded. ---
    // The converged in-band RMS is `target_rms`; a Gaussian random drive that realizes it
    // has bounded peaks. A runaway (servo climbing into a dead channel) would blow this up
    // by orders of magnitude. Bound the peak generously but finitely.
    assert!(
        result.max_drive_peak < 100.0,
        "drive ran away (peak {:.1}); target in-band RMS is {:.2} g",
        result.max_drive_peak,
        target_rms
    );

    // --- No spurious fault in the healthy case. ---
    // If the servo's dropout fault or drive ceiling ever tripped on this HEALTHY run, that
    // is a bug to investigate (the control channel is live and the level is in range).
    assert!(
        !result.unexpected_servo_event,
        "servo dropout fault or drive ceiling tripped on a healthy run"
    );
    // The engine issued no emergency-stops on the healthy converged loop.
    assert_eq!(
        result.estops, 0,
        "engine issued {} emergency-stop(s) on a healthy run",
        result.estops
    );
}

/// The converged in-band response RMS lands on the target profile's RMS (the servo's
/// overall-level job), not just the per-bin shape. A separate, coarser check than the
/// per-bin PSD error so a shape-vs-level regression is caught distinctly.
#[test]
fn converged_level_matches_target_rms() {
    let cfg = DemoConfig::default();
    let target_rms = cfg.target_inband_rms();
    let result = run_demo(cfg, 60);

    // Average the measured in-band RMS over the settled tail.
    let tail: Vec<f64> = result
        .trace
        .iter()
        .rev()
        .take(10)
        .filter_map(|s| s.inband_rms_g)
        .collect();
    assert!(!tail.is_empty(), "no settled RMS measurements");
    let mean_rms = tail.iter().sum::<f64>() / tail.len() as f64;

    // Within +/- 1.5 dB of the target RMS (a factor of ~1.19).
    let ratio_db = 20.0 * (mean_rms / target_rms).log10();
    assert!(
        ratio_db.abs() <= 1.5,
        "converged in-band RMS {mean_rms:.3} g is {ratio_db:+.2} dB off the target \
         {target_rms:.3} g"
    );
}

/// Reproducibility: the same config converges to the same steady-state error each run.
///
/// This is NOT bit-exact determinism: the plant (seeded noise) and the COLA drive (seeded)
/// are deterministic, but a segment that hits a spurious wall-clock ComputeOverrun is
/// re-run (see `run_segment`), and each retry consumes extra plant/drive RNG, so the exact
/// sample stream depends on timing. What IS reproducible is the CONTROL OUTCOME: both runs
/// settle to the same in-band error within the single-segment PSD-estimate variance.
#[test]
fn run_converges_reproducibly() {
    let a = run_demo(DemoConfig::default(), 30);
    let b = run_demo(DemoConfig::default(), 30);
    assert_eq!(a.trace.len(), b.trace.len());
    let ea = a.settled_mean_err_db(8).unwrap();
    let eb = b.settled_mean_err_db(8).unwrap();
    // Both settle near the same low error; allow the single-segment Welch PSD variance.
    assert!(
        ea < 3.0 && eb < 3.0,
        "a run did not converge: {ea:.2} / {eb:.2} dB"
    );
    assert!(
        (ea - eb).abs() < 1.5,
        "settled error not reproducible: {ea:.2} vs {eb:.2} dB"
    );
}

/// Without the adaptive spectral (shape) correction the loop still runs and stays
/// bounded and fault-free, exercising the pure FRF-inverse path (it just converges to a
/// looser shape). Guards the `spectral_correction = false` code path.
#[test]
fn pure_frf_inverse_path_is_stable() {
    let cfg = DemoConfig {
        spectral_correction: false,
        ..DemoConfig::default()
    };
    let result = run_demo(cfg, 40);

    assert!(
        result.abnormal_stop.is_none(),
        "aborted: {:?}",
        result.abnormal_stop
    );
    assert_eq!(result.estops, 0);
    assert!(!result.unexpected_servo_event);
    assert!(
        result.max_drive_peak < 100.0,
        "drive ran away without spectral correction (peak {:.1})",
        result.max_drive_peak
    );
    // The overall LEVEL still converges via the servo even with the shape loop off.
    let tail: Vec<f64> = result
        .trace
        .iter()
        .rev()
        .take(8)
        .filter_map(|s| s.inband_rms_g)
        .collect();
    let mean_rms = tail.iter().sum::<f64>() / tail.len() as f64;
    let target_rms = DemoConfig::default().target_inband_rms();
    let ratio_db = 20.0 * (mean_rms / target_rms).log10();
    assert!(
        ratio_db.abs() <= 2.0,
        "level did not converge on the pure-FRF path ({ratio_db:+.2} dB off target)"
    );
}
