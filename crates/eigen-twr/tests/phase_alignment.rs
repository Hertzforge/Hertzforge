//! PHASE-ALIGNMENT PINNING TEST (the TWR analog of eigen-dsp's
//! `h_estimate_hv_phase_sign_pinned`).
//!
//! The full pure-function pipeline - sysid estimate -> convention-bridged
//! regularized inverse -> initial drive -> time-domain plant simulation - must
//! land the response ON the target for a plain gain+delay plant. If the
//! convention bridge in `GridInverse::from_sysid` is dropped or double-applied,
//! the response arrives ~2*delay late (a near-inversion at mid-band
//! frequencies) and this test trips. See the CONVENTION BRIDGE comment in
//! `src/inverse.rs` for the algebra.

use eigen_twr::grid::{irfft_half, rfft_half, RecordGrid};
use eigen_twr::law;
use eigen_twr::{GridInverse, InverseConfig, LawConfig, SysIdConfig};

const FS: f64 = 4096.0;
const GAIN: f64 = 2.0;
const DELAY: usize = 7;

/// The plant: y[i] = GAIN * d[i - DELAY], fresh state per record.
fn plant(drive: &[f64]) -> Vec<f64> {
    (0..drive.len())
        .map(|i| {
            if i >= DELAY {
                GAIN * drive[i - DELAY]
            } else {
                0.0
            }
        })
        .collect()
}

#[test]
fn initial_drive_lands_the_response_on_the_target_not_two_delays_late() {
    // ---- sysid over the pure plant ----
    let sysid_cfg = SysIdConfig {
        drive_rms: 0.1,
        duration_s: 1.0,
        nfft: 512,
        f_lo_hz: 30.0,
        f_hi_hz: 1500.0,
        ..SysIdConfig::default()
    };
    let excitation = sysid_cfg.build_excitation(FS);
    let response = plant(&excitation);
    let sysid = sysid_cfg.estimate(&excitation, &response, FS).unwrap();
    assert!(sysid.in_band_mean_coherence > 0.99);

    // ---- a burst target on a padded grid ----
    let sec_n = 600usize;
    let section: Vec<f64> = (0..sec_n)
        .map(|i| {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / sec_n as f64).cos();
            w * (std::f64::consts::TAU * 200.0 * i as f64 / FS).sin()
        })
        .collect();
    let grid = RecordGrid::new(FS, sec_n, 128, 296); // n = 1024
    let target = grid.window_from_section(&section);
    let r = rfft_half(&target, grid.n);

    // ---- convention-bridged inverse + initial drive ----
    let inv =
        GridInverse::from_sysid(&sysid, &InverseConfig::default(), &grid, 30.0, 1500.0).unwrap();
    let law_cfg = LawConfig {
        f_lo_hz: 30.0,
        f_hi_hz: 1500.0,
        ..LawConfig::default()
    };
    let mut mask = law::build_mask(&r, &grid, &inv, &law_cfg);
    law::apply_coherence_gate(&mut mask, &inv, 0.9);
    let d0 = law::initial_drive(&r, &inv, &mask);
    let drive = irfft_half(&d0, grid.n);

    // ---- play through the TIME-DOMAIN plant and compare ----
    let achieved = plant(&drive);
    let err2: f64 = achieved
        .iter()
        .zip(target.iter())
        .map(|(&a, &t)| (a - t) * (a - t))
        .sum();
    let ref2: f64 = target.iter().map(|&t| t * t).sum();
    let rms_error_pct = 100.0 * (err2 / ref2).sqrt();
    assert!(
        rms_error_pct < 10.0,
        "initial drive misaligned: {rms_error_pct:.1}% rms error \
         (a ~2*delay-late response means the convention bridge broke)"
    );

    // And the cross-correlation peak sits at lag ZERO (aligned), not 2*DELAY.
    let mut best_lag = 0isize;
    let mut best = f64::MIN;
    for lag in -(2 * DELAY as isize + 4)..=(2 * DELAY as isize + 4) {
        let mut acc = 0.0f64;
        for (i, &t) in target.iter().enumerate() {
            let j = i as isize + lag;
            if j >= 0 && (j as usize) < grid.n {
                acc += t * achieved[j as usize];
            }
        }
        if acc > best {
            best = acc;
            best_lag = lag;
        }
    }
    assert_eq!(
        best_lag, 0,
        "response/target cross-correlation peaks at lag {best_lag}, not 0"
    );
}
