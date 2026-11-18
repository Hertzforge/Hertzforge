//! `matrix` and its pieces against vectors exported from the research testbed
//! (its interpolation, inverse, group delay, loop-speed rule and modal fit, and
//! whole closed-loop runs on its quasi-steady loop with the guards off).

mod common;
use common::*;
use hertzforge_mimo_sine_law_math::linalg::reg_pinv;
use hertzforge_mimo_sine_law_math::matrix::{MatrixLaw, MatrixParams, MATRIX_VALUES};
use hertzforge_mimo_sine_law_math::modal::{ModalForm, Model};
use hertzforge_mimo_sine_law_math::reference_structure::{
    run_closed_loop, Dynamics, Played, Schedule,
};
use hertzforge_mimo_sine_law_math::setup::GroupDelay;
use hertzforge_mimo_sine_law_math::speed::{gain, GainRule};
use hertzforge_mimo_sine_law_math::C64;

/// The model on the testbed's own modes (its fit exported), so a run compares
/// the law and not the fit.
fn testbed_model(case: &serde_json::Value, setup: &hertzforge_mimo_sine_law_math::Setup) -> Model {
    let modes = ModalForm {
        f_hz: f64s(&case["fit_f"]),
        zeta: f64s(&case["fit_z"]),
        residues: cplx(&case["fit_a"]),
    };
    Model::from_modes(&setup.identification, modes, 8).unwrap()
}

const CASES: [&str; 3] = ["lin4x4", "lin6x3", "lin3x6"];

#[test]
fn mimo_sine_parity_interpolation_inverse_gain_and_fit() {
    let data = load("matrix_parity.json");
    for name in CASES {
        let case = &data[name];
        let st = structure(case);
        let setup = st.setup(band(case), true);
        let (n, m) = (st.controls, st.drives);
        let fitted = Model::build(&setup.identification, band(case), Default::default()).unwrap();
        // The modal fit. Same candidates, same starting point and the same
        // starting cost (to 2e-15), but a group of many candidates fitted
        // jointly (eight on the 4 by 4, most of them scatter that the fit then
        // drops for damping over 0.2) follows a different Levenberg-Marquardt
        // path from rounding alone. So: the same modes, each frequency within
        // 2 percent of its half-power bandwidth, each damping within 25 percent.
        let ff = f64s(&case["fit_f"]);
        let fz = f64s(&case["fit_z"]);
        assert_eq!(fitted.modes.f_hz.len(), ff.len(), "{name}: modes found");
        for r in 0..ff.len() {
            let bw = 2.0 * fz[r] * ff[r];
            let df = (fitted.modes.f_hz[r] - ff[r]).abs() / bw;
            let dz = (fitted.modes.zeta[r] / fz[r] - 1.0).abs();
            eprintln!(
                "{name} mode {r}: {:.3} Hz, shift {df:.4} bandwidths, damping {dz:.3}",
                ff[r]
            );
            assert!(df < 0.02 && dz < 0.25, "{name} mode {r}: {df} {dz}");
        }
        let model = testbed_model(case, &setup);
        let gd = GroupDelay::of(&setup.identification);
        for p in case["probes"].as_array().unwrap() {
            let f = p["f"].as_f64().unwrap();
            let mut h = vec![C64::new(0.0, 0.0); n * m];
            setup.response_at(f, &mut h);
            assert!(rel_err(&h, &cplx(&p["H0"])) < 1e-12, "{name} H0 at {f}");
            let (z, _) = reg_pinv(&h, n, m, 1e-3).unwrap();
            assert!(rel_err(&z, &cplx(&p["Z0"])) < 1e-9, "{name} Z0 at {f}");
            model.at(f, &mut h);
            assert!(
                rel_err(&h, &cplx(&p["model"])) < 1e-9,
                "{name} model at {f}"
            );
            let b = setup.tracking.bandwidth_hz(f);
            assert!((gd.at(f, 2.0 * b) / p["gd"].as_f64().unwrap() - 1.0).abs() < 1e-9);
            assert!((gd.at(f, 0.0) / p["gd_local"].as_f64().unwrap() - 1.0).abs() < 1e-9);
            let g = gain(
                testbed_rule(),
                &setup.tracking,
                setup.loop_delay,
                &gd,
                setup.tick_s,
                f,
            )
            .g;
            assert!(
                (g / p["gain"].as_f64().unwrap() - 1.0).abs() < 1e-9,
                "{name} gain at {f}"
            );
        }
    }
}

#[test]
fn mimo_sine_parity_matrix_closed_loop_runs() {
    let data = load("matrix_parity.json");
    for name in CASES {
        let case = &data[name];
        let st = structure(case);
        let setup = st.setup(band(case), true);
        for (kind, run) in case["runs"].as_object().unwrap() {
            let sched = match (name, kind.as_str()) {
                ("lin4x4", "dwell") => Schedule::Dwell { f: 69.0, secs: 1.5 },
                ("lin4x4", "steps") => Schedule::Steps {
                    f0: 40.0,
                    f1: 56.0,
                    per_octave: 12.0,
                    secs: 0.4,
                },
                ("lin4x4", "sweep") => Schedule::Sweep {
                    f0: 60.0,
                    f1: 85.0,
                    oct_min: 1.0,
                },
                ("lin6x3", _) => Schedule::Sweep {
                    f0: 40.0,
                    f1: 70.0,
                    oct_min: 1.0,
                },
                _ => Schedule::Sweep {
                    f0: 60.0,
                    f1: 90.0,
                    oct_min: 1.0,
                },
            };
            let model = testbed_model(case, &setup);
            let p = MatrixParams {
                gain_rule: testbed_rule(),
                ..Default::default()
            };
            let mut law = MatrixLaw::with_model(&setup, p, model).unwrap();
            let rows = run_closed_loop(
                &mut law,
                &st,
                &setup,
                sched,
                Played::AsAsked,
                Dynamics::QuasiSteady,
                MATRIX_VALUES.len(),
                100_000,
            )
            .unwrap();
            assert_eq!(
                rows.len() as u64,
                run["updates"].as_u64().unwrap(),
                "{name} {kind}: updates"
            );
            let idx = run["index"].as_array().unwrap();
            let mut worst = (0.0f64, 0.0f64);
            for (j, i) in idx.iter().enumerate() {
                let i = i.as_u64().unwrap() as usize;
                let row = &rows[i];
                assert!(
                    (row.f / run["f"][j].as_f64().unwrap() - 1.0).abs() < 1e-9,
                    "{name} {kind} f at {i}"
                );
                worst.0 = worst.0.max(rel_err(&row.drive, &cplx(&run["D"][j])));
                if i > 0 {
                    worst.1 = worst.1.max(rel_err(&row.controls, &cplx(&run["C"][j])));
                }
            }
            eprintln!(
                "{name} {kind}: worst relative drive {:.2e}, controls {:.2e}",
                worst.0, worst.1
            );
            // Rounding only: the same arithmetic over up to 4,841 updates.
            assert!(worst.0 < 1e-9 && worst.1 < 1e-9, "{name} {kind}: {worst:?}");
        }
    }
}

#[test]
fn mimo_sine_parity_matrix_amplitude_only_reference() {
    use hertzforge_mimo_sine_law_math::matrix::FreePhase;
    let data = load("matrix_parity.json");
    let amp = load("amplitude_parity.json");
    let case = &data["lin4x4"];
    let st = structure(case);
    let mut setup = st.setup(band(case), true);
    setup.phase_specified = false;
    for (key, sched, fp) in [
        (
            "dwell/in_phase",
            Schedule::Dwell { f: 69.0, secs: 1.5 },
            FreePhase::InPhase,
        ),
        (
            "sweep/in_phase",
            Schedule::Sweep {
                f0: 60.0,
                f1: 85.0,
                oct_min: 1.0,
            },
            FreePhase::InPhase,
        ),
        (
            "dwell/measured",
            Schedule::Dwell { f: 69.0, secs: 1.5 },
            FreePhase::Measured,
        ),
        (
            "sweep/measured",
            Schedule::Sweep {
                f0: 60.0,
                f1: 85.0,
                oct_min: 1.0,
            },
            FreePhase::Measured,
        ),
    ] {
        let run = &amp[key];
        let p = MatrixParams {
            free_phase: fp,
            gain_rule: testbed_rule(),
            ..Default::default()
        };
        let mut law = MatrixLaw::with_model(&setup, p, testbed_model(case, &setup)).unwrap();
        let rows = run_closed_loop(
            &mut law,
            &st,
            &setup,
            sched,
            Played::AsAsked,
            Dynamics::QuasiSteady,
            MATRIX_VALUES.len(),
            100_000,
        )
        .unwrap();
        assert_eq!(rows.len() as u64, run["updates"].as_u64().unwrap(), "{key}");
        let mut worst = 0.0f64;
        for (j, i) in run["index"].as_array().unwrap().iter().enumerate() {
            let row = &rows[i.as_u64().unwrap() as usize];
            worst = worst.max(rel_err(&row.drive, &cplx(&run["D"][j])));
            worst = worst.max(rel_err(&row.controls, &cplx(&run["C"][j])));
        }
        eprintln!("amplitude only {key}: worst relative difference {worst:.2e}");
        assert!(worst < 1e-9, "{key}: {worst}");
    }
}
