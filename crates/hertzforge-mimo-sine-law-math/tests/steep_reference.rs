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

/// Requirement: on a structure close to flat, at one octave a minute,
/// `matrix` holds the response within 1 dB over the reference through a
/// 12 dB rise or fall over 1 Hz or over 0.1 Hz (the identification's lines
/// are 0.25 Hz apart). Before the fix: up to 24.9 dB.
#[test]
fn mimo_sine_matrix_holds_a_steep_reference_on_a_flat_structure() {
    for rise in [true, false] {
        for width in [1.0, 0.1] {
            let (a, _) = sweep_case(&flat(), MatrixParams::default(), 1.0, rise, width);
            println!(
                "rise {rise} width {width}: true over {:+.2} dB",
                a.true_over_db
            );
            assert!(a.true_over_db < 1.0, "rise {rise} width {width}: {a:?}");
        }
    }
}

/// The correction compares the tracked controls with the reference as the
/// trackers read it: without that, the trackers' trail behind a moving
/// reference is integrated as an error (measured 1.98 dB over on the 0.1 Hz
/// rise against 0.52 with it).
#[test]
fn mimo_sine_matrix_corrects_on_the_reference_as_the_trackers_read_it() {
    let with = sweep_case(&flat(), MatrixParams::default(), 1.0, true, 0.1).0;
    let without = sweep_case(
        &flat(),
        MatrixParams {
            seen_reference: false,
            ..MatrixParams::default()
        },
        1.0,
        true,
        0.1,
    )
    .0;
    println!(
        "seen reference: {:+.2} dB; reference now: {:+.2} dB",
        with.true_over_db, without.true_over_db
    );
    assert!(with.true_over_db < 0.75, "{with:?}");
    assert!(without.true_over_db > 1.5, "{without:?}");
}

/// Updates after the told reference falls 12 dB at once until every control
/// is within 1 dB of it, with the drive in force held to a 1 dB change per
/// update (the controller's default change cap) and the structure answering
/// at once.
fn updates_to_follow_a_capped_fall(params: MatrixParams) -> usize {
    use hertzforge_mimo_sine_law_math::law::{Hold, Inputs, Outputs};
    let st = flat();
    let s = st.setup((30.0, 120.0), false);
    let (n, m) = (s.controls, s.drives);
    let f = 60.0;
    let mut h = vec![C64::new(0.0, 0.0); n * m];
    st.frf(f, &mut h);
    let mut law = MatrixLaw::new(&s, params).expect("builds");
    let mut out = Outputs::new(m, MATRIX_VALUES.len());
    let mut d = vec![C64::new(0.0, 0.0); m];
    let mut c = vec![C64::new(0.0, 0.0); n];
    let cap = 10f64.powf(1.0 / 20.0);
    let fall_at = 50;
    let mut within_at = None;
    for k in 0..400 {
        let level = if k < fall_at { 1.0 } else { 0.25 };
        let r = vec![C64::new(level, 0.0); n];
        let x = Inputs {
            index: k as u64,
            dt_s: if k == 0 { 0.0 } else { s.tick_s },
            f_hz: f,
            sweep_hz_s: 0.0,
            sweep_oct_min: 0.0,
            level: 1.0,
            reference: &r,
            controls: &c,
            limits: &[],
            limit_bounds: &[],
            drive_in_force: &d,
            hold: if k == 0 { Hold::Engage } else { Hold::Free },
            notch_scale: 1.0,
            settle_open: false,
        };
        if k == 0 {
            law.engage(&x, &mut out).unwrap();
            d.copy_from_slice(&out.drive);
        } else {
            law.tick(&x, &mut out).unwrap();
            for (di, ai) in d.iter_mut().zip(&out.drive) {
                let (a0, a1) = (di.norm(), ai.norm());
                let mag = a1.clamp(a0 / cap, a0 * cap);
                *di = if a1 > 0.0 { ai * (mag / a1) } else { *ai };
            }
        }
        for (i, ci) in c.iter_mut().enumerate() {
            *ci = (0..m).map(|j| h[i * m + j] * d[j]).sum();
        }
        if k > fall_at && within_at.is_none() {
            let worst = c
                .iter()
                .map(|v| (20.0 * (v.norm() / 0.25).log10()).abs())
                .fold(0.0, f64::max);
            if worst < 1.0 {
                within_at = Some(k - fall_at);
            }
        }
    }
    within_at.unwrap_or(usize::MAX)
}

/// A fall faster than the change cap allows is followed at the cap's pace:
/// the part of the carry the cap cut is carried again, not left to the
/// correction.
#[test]
fn mimo_sine_matrix_carries_again_what_the_change_cap_cut() {
    let with = updates_to_follow_a_capped_fall(MatrixParams::default());
    let without = updates_to_follow_a_capped_fall(MatrixParams {
        recarry: false,
        ..MatrixParams::default()
    });
    println!("within 1 dB after {with} updates (carried again), {without} (not)");
    assert!(with <= 13, "{with}");
    assert!(without >= 25, "{without}");
}
