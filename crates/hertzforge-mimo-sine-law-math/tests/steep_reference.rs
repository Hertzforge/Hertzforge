//! A reference that moves faster than the loop can see: a 12 dB rise or fall
//! over 1 Hz or 0.1 Hz met in a sweep, on the reference structure with its
//! envelope dynamics (each mode builds up, the tracking filter lags).
//!
//! The evidence print: `cargo test -p hertzforge-mimo-sine-law-math --test
//! steep_reference -- --ignored --nocapture`.

use hertzforge_mimo_sine_law_math::law::SineLaw;
use hertzforge_mimo_sine_law_math::matrix::{MatrixLaw, MatrixParams, MATRIX_VALUES};
use hertzforge_mimo_sine_law_math::reference_structure::{
    run_closed_loop_with, sample_reference, stepped_reference, Dynamics, ModalStructure, Played,
    Row, Schedule,
};
use hertzforge_mimo_sine_law_math::setup::Setup;
use hertzforge_mimo_sine_law_math::C64;

/// Two drives, two controls, modes well clear of the step: close to flat in
/// 40 to 80 Hz (a mass line), lightly coupled.
fn flat() -> ModalStructure {
    ModalStructure {
        f_hz: vec![4.0, 4.5, 300.0, 340.0],
        zeta: vec![0.05, 0.05, 0.02, 0.03],
        drive_shape: vec![1.0, 0.1, 0.1, 1.0, 0.8, 0.1, 0.1, 0.7],
        output_shape: vec![1.0, 0.1, 0.1, 1.0, 0.6, 0.1, 0.1, 0.5],
        drives: 2,
        controls: 2,
    }
}

/// The same with a light mode at 66 Hz, just above the step.
fn resonant() -> ModalStructure {
    ModalStructure {
        f_hz: vec![4.0, 4.5, 66.0, 85.0],
        zeta: vec![0.05, 0.05, 0.02, 0.03],
        ..flat()
    }
}

/// What a run did around the step: the largest response over the reference
/// (by the structure itself, `H D` of the drive played), the largest reading
/// over and under it (the tracked amplitude), and the largest drive asked
/// over the largest drive played.
#[derive(Debug, Clone, Copy)]
pub struct Around {
    pub true_over_db: f64,
    pub read_over_db: f64,
    pub read_under_db: f64,
}

fn around(st: &ModalStructure, rows: &[Row], r: &dyn Fn(f64) -> f64, band: (f64, f64)) -> Around {
    let (n, m) = (st.controls, st.drives);
    let mut h = vec![C64::new(0.0, 0.0); n * m];
    let mut a = Around {
        true_over_db: f64::NEG_INFINITY,
        read_over_db: f64::NEG_INFINITY,
        read_under_db: f64::INFINITY,
    };
    for row in rows.iter().filter(|w| w.f >= band.0 && w.f <= band.1) {
        st.frf(row.f, &mut h);
        let rf = r(row.f);
        for i in 0..n {
            let y: C64 = (0..m).map(|j| h[i * m + j] * row.drive[j]).sum();
            a.true_over_db = a.true_over_db.max(20.0 * (y.norm() / rf).log10());
            let c = row.controls[i].norm().max(1e-300);
            let e = 20.0 * (c / rf).log10();
            a.read_over_db = a.read_over_db.max(e);
            a.read_under_db = a.read_under_db.min(e);
        }
    }
    a
}

pub fn sweep_case(
    st: &ModalStructure,
    params: MatrixParams,
    rate: f64,
    rise: bool,
    width: f64,
) -> (Around, Vec<Row>) {
    let (lo, hi) = if width == 0.0 {
        (1.0, 1.0)
    } else if rise {
        (0.25, 1.0)
    } else {
        (1.0, 0.25)
    };
    let width = if width == 0.0 { 1.0 } else { width };
    let f2 = 60.0;
    let r = stepped_reference(lo, hi, f2 - width, f2);
    let mut s: Setup = st.setup((30.0, 120.0), false);
    sample_reference(&mut s, &r);
    let mut law = MatrixLaw::new(&s, params).expect("builds");
    let rows = run(&mut law, st, &s, rate, &r);
    (around(st, &rows, &r, (f2 - width - 0.5, f2 + 3.0)), rows)
}

fn run(
    law: &mut dyn SineLaw,
    st: &ModalStructure,
    s: &Setup,
    rate: f64,
    r: &dyn Fn(f64) -> f64,
) -> Vec<Row> {
    run_closed_loop_with(
        law,
        st,
        s,
        Schedule::Sweep {
            f0: 40.0,
            f1: 80.0,
            oct_min: rate,
        },
        Played::AsAsked,
        Dynamics::Envelope,
        MATRIX_VALUES.len(),
        200_000,
        r,
    )
    .expect("runs")
}

#[test]
#[ignore]
fn mimo_sine_steep_reference_math_evidence() {
    println!("structure | carry | rate oct/min | step | width Hz | true over dB | read over dB | read under dB");
    for (name, st) in [("flat", flat()), ("resonant", resonant())] {
        for carry in [0.67, 0.0] {
            for rate in [1.0, 4.0] {
                for rise in [true, false] {
                    for width in [1.0, 0.1, 0.0] {
                        if width == 0.0 && !rise {
                            continue;
                        }
                        let p = MatrixParams {
                            carry,
                            ..MatrixParams::default()
                        };
                        let (a, rows) = sweep_case(&st, p, rate, rise, width);
                        println!(
                            "{name} | {carry} | {rate} | {} | {width} | {:+.2} | {:+.2} | {:+.2}",
                            if rise { "rise" } else { "fall" },
                            a.true_over_db,
                            a.read_over_db,
                            a.read_under_db
                        );
                        if std::env::var("STEEP_TRACE").is_ok() && carry > 0.0 && rate == 1.0 {
                            let r = stepped_reference(
                                if rise { 0.25 } else { 1.0 },
                                if rise { 1.0 } else { 0.25 },
                                60.0 - width,
                                60.0,
                            );
                            for w in rows
                                .iter()
                                .filter(|w| w.f > 59.0 - width && w.f < 61.0)
                                .step_by(4)
                            {
                                println!(
                                    "    {:7.3} R {:.4} C {:.4} D {:.4} asked {:.4} gain {:.4}",
                                    w.f,
                                    r(w.f),
                                    w.controls[0].norm(),
                                    w.drive[0].norm(),
                                    w.asked[0].norm(),
                                    w.values[0]
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
