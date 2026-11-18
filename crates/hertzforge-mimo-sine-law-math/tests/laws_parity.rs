//! `steepest_descent`, `secant` (both carries), `ratio` (both pairings) and
//! `paired` against vectors exported from the research testbed: a dwell and a
//! stepped sine on the linear 4 by 4, its quasi-steady loop, guards off.

mod common;
use common::*;
use hertzforge_mimo_sine_law_math::law::SineLaw;
use hertzforge_mimo_sine_law_math::paired::{PairedLaw, PairedParams};
use hertzforge_mimo_sine_law_math::ratio::{Pairing, RatioLaw, RatioParams};
use hertzforge_mimo_sine_law_math::reference_structure::{
    run_closed_loop, Dynamics, Played, Schedule,
};
use hertzforge_mimo_sine_law_math::secant::{SecantCarry, SecantLaw, SecantParams};
use hertzforge_mimo_sine_law_math::steepest::{SteepestLaw, SteepestParams};
use hertzforge_mimo_sine_law_math::Setup;

fn make(name: &str, setup: &Setup) -> Box<dyn SineLaw> {
    match name {
        "steepest_descent" => Box::new(SteepestLaw::new(setup, SteepestParams::default()).unwrap()),
        "secant_factor" => Box::new(SecantLaw::new(setup, SecantParams::default()).unwrap()),
        "secant_plain" => Box::new(
            SecantLaw::new(
                setup,
                SecantParams {
                    carry: SecantCarry::Plain,
                    ..Default::default()
                },
            )
            .unwrap(),
        ),
        "ratio_reference" => Box::new(RatioLaw::new(setup, RatioParams::default()).unwrap()),
        "ratio_drive" => Box::new(
            RatioLaw::new(
                setup,
                RatioParams {
                    pairing: Pairing::Drive,
                    ..Default::default()
                },
            )
            .unwrap(),
        ),
        _ => Box::new(PairedLaw::new(setup, PairedParams::diagonal(4, 4)).unwrap()),
    }
}

fn check(law_name: &str) {
    let fx = load("matrix_parity.json");
    let case = &fx["lin4x4"];
    let st = structure(case);
    let setup = st.setup(band(case), true);
    let data = load("laws_parity.json");
    for (kind, sched) in [
        ("dwell", Schedule::Dwell { f: 69.0, secs: 2.0 }),
        (
            "steps",
            Schedule::Steps {
                f0: 40.0,
                f1: 56.0,
                per_octave: 12.0,
                secs: 0.4,
            },
        ),
    ] {
        let run = &data[format!("{law_name}/{kind}")];
        let mut law = make(law_name, &setup);
        let nv = law.value_names().len();
        let rows = run_closed_loop(
            law.as_mut(),
            &st,
            &setup,
            sched,
            Played::AsAsked,
            Dynamics::QuasiSteady,
            nv,
            10_000,
        )
        .unwrap();
        assert_eq!(
            rows.len() as u64,
            run["updates"].as_u64().unwrap(),
            "{law_name} {kind}"
        );
        let mut worst = 0.0f64;
        for (j, i) in run["index"].as_array().unwrap().iter().enumerate() {
            let row = &rows[i.as_u64().unwrap() as usize];
            worst = worst.max(rel_err(&row.drive, &cplx(&run["D"][j])));
            worst = worst.max(rel_err(&row.controls, &cplx(&run["C"][j])));
        }
        eprintln!("{law_name} {kind}: worst relative difference {worst:.2e}");
        // Rounding only: the same arithmetic over at most 241 updates.
        assert!(worst < 1e-9, "{law_name} {kind}: {worst}");
    }
}

#[test]
fn mimo_sine_parity_steepest_descent_dwell_and_steps() {
    check("steepest_descent");
}

#[test]
fn mimo_sine_parity_secant_factor_dwell_and_steps() {
    check("secant_factor");
}

#[test]
fn mimo_sine_parity_secant_plain_dwell_and_steps() {
    check("secant_plain");
}

#[test]
fn mimo_sine_parity_ratio_reference_dwell_and_steps() {
    check("ratio_reference");
}

#[test]
fn mimo_sine_parity_ratio_drive_dwell_and_steps() {
    check("ratio_drive");
}

#[test]
fn mimo_sine_parity_paired_dwell_and_steps() {
    check("paired");
}
