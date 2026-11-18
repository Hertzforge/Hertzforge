//! Behaviour the parity runs do not show: the amplitude-only reference, the
//! `paired` template on a structure it suits, and the readings.

mod common;
use common::*;
use hertzforge_mimo_sine_law_math::matrix::{FreePhase, MatrixLaw, MatrixParams, MATRIX_VALUES};
use hertzforge_mimo_sine_law_math::modal::ModalForm;
use hertzforge_mimo_sine_law_math::paired::{PairedLaw, PairedParams, PAIRED_VALUES};
use hertzforge_mimo_sine_law_math::readings::{
    largest_share, opposition_ratio, sweep_rate_warning,
};
use hertzforge_mimo_sine_law_math::reference_structure::{
    run_closed_loop, Dynamics, ModalStructure, Played, Schedule,
};
use hertzforge_mimo_sine_law_math::C64;

/// With an amplitude-only reference and `FreePhase::Measured` each control's phase is taken from its
/// measurement: controls at the reference's amplitude with any phases ask for
/// no correction at a dwell. With the phases specified the same reading is an
/// error and moves the drive.
fn step_norm(phase_specified: bool, free_phase: FreePhase) -> f64 {
    use hertzforge_mimo_sine_law_math::law::{Hold, Inputs, Outputs, SineLaw};
    let data = load("matrix_parity.json");
    let st = structure(&data["lin4x4"]);
    let mut setup = st.setup(band(&data["lin4x4"]), true);
    setup.phase_specified = phase_specified;
    let p = MatrixParams {
        free_phase,
        ..Default::default()
    };
    let mut law = MatrixLaw::new(&setup, p).unwrap();
    let r = vec![C64::new(1.0, 0.0); 4];
    let c: Vec<C64> = (0..4)
        .map(|i| C64::from_polar(1.0, 0.7 * i as f64))
        .collect();
    let d = vec![C64::new(0.3, 0.1); 4];
    let x = Inputs {
        index: 50,
        dt_s: 0.01,
        f_hz: 69.0,
        sweep_hz_s: 0.0,
        sweep_oct_min: 0.0,
        level: 1.0,
        reference: &r,
        controls: &c,
        limits: &[],
        limit_bounds: &[],
        drive_in_force: &d,
        hold: Hold::Free,
        notch_scale: 1.0,
        settle_open: false,
    };
    let mut out = Outputs::new(4, MATRIX_VALUES.len());
    law.tick(&x, &mut out).unwrap();
    out.drive
        .iter()
        .zip(&d)
        .map(|(a, b)| (a - b).norm_sqr())
        .sum::<f64>()
        .sqrt()
}

#[test]
fn mimo_sine_amplitude_only_reference_takes_phases_from_the_measurement() {
    let free = step_norm(false, FreePhase::Measured);
    let in_phase = step_norm(false, FreePhase::InPhase);
    let fixed = step_norm(true, FreePhase::Measured);
    eprintln!("controls on amplitude, phases spread: step {free:.2e} measured, {in_phase:.2e} in phase, {fixed:.2e} phases specified");
    assert!(free < 1e-12, "{free}");
    assert!(in_phase > 1e-3, "{in_phase}");
    assert!(
        (in_phase - fixed).abs() < 1e-12,
        "in phase is the specified reference of ones"
    );
}

#[test]
fn mimo_sine_paired_holds_a_dwell_on_weakly_coupled_pairs() {
    // Two drives, two controls, each mode seen mostly by its own pair.
    let st = ModalStructure {
        f_hz: vec![4.0, 4.5, 60.0, 85.0],
        zeta: vec![0.05, 0.05, 0.02, 0.03],
        drive_shape: vec![1.0, 0.05, 0.05, 1.0, 0.8, 0.05, 0.05, 0.7],
        output_shape: vec![1.0, 0.05, 0.05, 1.0, 0.6, 0.05, 0.05, 0.5],
        drives: 2,
        controls: 2,
    };
    let setup = st.setup((30.0, 120.0), false);
    let mut law = PairedLaw::new(&setup, PairedParams::diagonal(2, 2)).unwrap();
    let dwell = Schedule::Dwell { f: 61.0, secs: 8.0 };
    let rows = run_closed_loop(
        &mut law,
        &st,
        &setup,
        dwell,
        Played::AsAsked,
        Dynamics::Envelope,
        PAIRED_VALUES.len(),
        10_000,
    )
    .unwrap();
    let worst = |k: usize| {
        rows[k]
            .controls
            .iter()
            .map(|c| (20.0 * c.norm().log10()).abs())
            .fold(0.0, f64::max)
    };
    for k in [100, 200, 400, 600, rows.len() - 1] {
        eprintln!("paired at {k}: {:.4} dB", worst(k));
    }
    // It converges, slowly: the error falls by about half every two seconds
    // at this mode (0.67 dB at 4 s, 0.25 dB at 8 s).
    let checks = [200, 400, 600, rows.len() - 1].map(worst);
    assert!(checks.windows(2).all(|w| w[1] < 0.8 * w[0]), "{checks:?}");
    assert!(checks[3] < 0.3, "{checks:?}");
}

#[test]
fn mimo_sine_readings_opposition_share_and_sweep_rate() {
    let one = C64::new(1.0, 0.0);
    let zero = C64::new(0.0, 0.0);
    // One control, two drives: [1 1]. The least-norm drive for any response is
    // along [1 1]; [1 1] is 1, [2 0] is sqrt(2), [1 -1] gives no response.
    let h = [one, one];
    assert!((opposition_ratio(&h, 1, 2, &[one, one]).unwrap() - 1.0).abs() < 1e-12);
    assert!((opposition_ratio(&h, 1, 2, &[one * 2.0, zero]).unwrap() - 2f64.sqrt()).abs() < 1e-12);
    assert_eq!(largest_share(&[one, one]), (0.5, 0));
    assert_eq!(largest_share(&[zero, one * 3.0]), (1.0, 1));
    // Q 50 (zeta 0.01): at 1 oct/min 1.1 time constants at 10 Hz (flagged),
    // 4.3 at 40 Hz (not); 1.7 s inside the bandwidth.
    let modes = ModalForm {
        f_hz: vec![10.0, 40.0],
        zeta: vec![0.01, 0.01],
        residues: vec![zero; 2],
    };
    let w = sweep_rate_warning(&modes, 1.0);
    assert!((w[0].time_constants - 1.088).abs() < 0.01 && w[0].flagged);
    assert!((w[1].time_constants - 4.35).abs() < 0.01 && !w[1].flagged);
    assert!((w[0].seconds_in_bandwidth - 1.73).abs() < 0.01);
}
