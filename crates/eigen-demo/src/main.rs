//! `eigen-demo` binary: run the Random-on-Sim closed loop and print a convergence trace.
//!
//! The output is a per-control-update table: iteration, phase, overall servo level (dB),
//! the in-band RMS error between the measured and target control PSD (dB), the measured
//! in-band RMS (g), and the peak drive commanded that window. A converging run shows the
//! error shrink from a large pre-test value toward ~0 dB and settle, with the drive
//! staying bounded.

use eigen_demo::{run_demo, DemoConfig, Phase};

fn phase_str(p: Phase) -> &'static str {
    match p {
        Phase::PreTest => "PRETEST",
        Phase::RampUp => "RAMPUP ",
        Phase::Hold => "HOLD   ",
    }
}

fn main() {
    let cfg = DemoConfig::default();

    println!("Eigen - Random-on-Sim closed-loop demo (zero hardware)");
    println!("=======================================================");
    println!(
        "plant: fs={:.0} Hz, modes={:?}, noise_sd={:.0e}",
        cfg.fs_hz, cfg.resonances, cfg.noise_level
    );
    println!(
        "control: band {:.0}-{:.0} Hz, flat target {:.1e} g^2/Hz, nfft={}, overlap={:.0}%",
        cfg.band_lo_hz,
        cfg.band_hi_hz,
        cfg.target_psd,
        cfg.nfft,
        cfg.overlap * 100.0
    );

    // Control windows: 3 pre-test + a few ramp + a long hold. One window is one analysis
    // segment run through the engine at frames_per_tick = 1.
    let max_windows = 60;
    let result = run_demo(cfg.clone(), max_windows);

    println!(
        "target in-band response RMS: {:.4} g (sqrt of profile integral)",
        result.target_inband_rms
    );
    println!();
    println!(" iter  phase    level(dB)   err(dB)   meas_rms(g)   drive_pk");
    println!(" ----  -------  ---------  ---------  -----------  ---------");
    for s in &result.trace {
        let err = s
            .inband_err_db
            .map(|e| format!("{e:+8.2}"))
            .unwrap_or_else(|| "     -- ".to_string());
        let rms = s
            .inband_rms_g
            .map(|r| format!("{r:11.4}"))
            .unwrap_or_else(|| "         --".to_string());
        println!(
            " {:>4}  {}  {:>9.2}  {}  {}  {:>9.3}",
            s.iter,
            phase_str(s.phase),
            s.level_db,
            err,
            rms,
            s.drive_peak
        );
    }
    println!();

    // Summary.
    let settled_mean = result.settled_mean_err_db(5);
    let settled_worst = result.settled_worst_err_db(5);
    println!("Run summary");
    println!("-----------");
    println!("  control windows    : {}", result.trace.len());
    println!(
        "  frames in / capt   : {} / {}",
        result.total_frames, result.total_captured
    );
    println!("  e-stops            : {}", result.estops);
    println!(
        "  abnormal stop      : {}",
        result
            .abnormal_stop
            .as_deref()
            .unwrap_or("none (all segments ended cleanly)")
    );
    println!(
        "  final phase        : {}",
        phase_str(result.final_phase).trim()
    );
    println!("  hold windows       : {}", result.hold_windows);
    println!("  max drive peak     : {:.3}", result.max_drive_peak);
    println!(
        "  servo fault/ceil   : {}",
        if result.unexpected_servo_event {
            "YES (bug!)"
        } else {
            "no"
        }
    );
    match (settled_mean, settled_worst) {
        (Some(m), Some(w)) => {
            println!("  settled mean |err| : {m:+.2} dB (last 5 updates)");
            println!("  settled worst |err|: {w:.2} dB (last 5 updates)");
            if w <= 3.0 {
                println!();
                println!("CONVERGED: in-band response tracks the target within 3 dB.");
            } else {
                println!();
                println!("NOT converged to 3 dB (worst {w:.2} dB) - see trace above.");
            }
        }
        _ => println!("  (no valid error measurements)"),
    }
}
