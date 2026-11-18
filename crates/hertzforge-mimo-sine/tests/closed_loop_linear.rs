//! Closed loop with `matrix`: a dwell, a stepped sine and a sweep at one
//! octave a minute on `sine-linear-4x4`, `sine-tall-6x3` and `sine-wide-3x6`,
//! and a sweep on `sine-softening-4x4-mild`, every input with measurement
//! noise ([`common::NOISE`]).
//!
//! Tolerances, on the worst error of any control over every judged update
//! (outside settle windows):
//!
//! - dwell 0.25 dB: at a dwell the inverse correction converges to the noise
//!   floor; anything larger is a loop that has not converged.
//! - stepped 1.5 dB, the profile's alarm band: each step starts on the law's
//!   engage at the new frequency and is judged only after its settle window.
//! - sweep 3 dB worst and 0.5 dB median: 3 dB is where the linear law's
//!   stated limit begins (errors over 3 dB are expected only once a resonance
//!   has moved a quarter of its bandwidth), and the median holds the loop to
//!   its alarm band away from the resonances.
//!
//! - With more controls than drives (`sine-tall-6x3`) the error is judged
//!   against the response the drives can reach (the least-squares fit through
//!   the identification), with dwell 0.5 dB and sweep 4 dB: the loop's fixed
//!   point is the least-squares fit through the law's model (the modes found
//!   plus the smoothed remainder), not through the raw identification, and the
//!   two differ by up to a few tenths of a dB at a dwell and about a decibel at
//!   a resonance in a sweep.
//!
//! Measured on this branch (the test prints them): linear 4 x 4 dwell 0.02,
//! stepped 0.70, sweep 1.29 worst and 0.11 median.

mod common;

use common::*;
use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_mimo_sine::records::Outcome;
use hertzforge_mimo_sine::*;
use hertzforge_sine::scheduler::{
    BidirSweepConfig, DwellFor, StepSineConfig, StepSpacing, SweepDirection, SweepTarget,
};
use hertzforge_sine::SweepRate;

pub fn stepped() -> TestPlan {
    TestPlan::Stepped(StepPlan::Grid(StepSineConfig {
        band: (30.0, 120.0),
        steps_per_octave: 12.0,
        spacing: StepSpacing::Log,
        dwell: Some(DwellFor::Seconds(1.5)),
    }))
}

pub fn sweep(band: (f64, f64)) -> TestPlan {
    TestPlan::Sweep(BidirSweepConfig {
        band,
        rate: SweepRate::LogOctPerMin(1.0),
        start_hz: band.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(1),
    })
}

/// Run `test` on `s` with `matrix`; the outcome, the median and the worst
/// error, dB. With more controls than drives the reference cannot be met at
/// every control; there the error is judged against the response the
/// identification says the drives can reach (`H H+ R`, the least-squares
/// fit), which is what the loop converges to.
fn case(s: SineStructure, test: TestPlan, max_s: f64) -> (Option<Outcome>, f64, f64) {
    let svc = engaged(s, setup(s, test, "matrix"), law("matrix"), NOISE);
    let id = svc.identification().unwrap().identification.clone();
    let tall = s.n_controls() > s.dims().1;
    let r = run(svc, max_s);
    let (n, m) = (s.n_controls(), s.dims().1);
    let reach = |t: &records::TickRecord| -> Vec<f64> {
        let mut h = vec![C64::new(0.0, 0.0); n * m];
        hertzforge_mimo_sine_law_math::setup::interp_stack(
            &id.frequency_hz,
            id.response.data(),
            n * m,
            t.frequency_hz,
            &mut h,
        );
        let (z, _) = hertzforge_mimo_sine_law_math::linalg::reg_pinv(&h, n, m, 1e-3).unwrap();
        let r: Vec<C64> = t.reference[..n]
            .iter()
            .map(|v| C64::new(v.norm(), 0.0))
            .collect();
        let mut d = vec![C64::new(0.0, 0.0); m];
        hertzforge_mimo_sine_law_math::linalg::mat_vec(&z, m, n, &r, &mut d);
        let mut c = vec![C64::new(0.0, 0.0); n];
        hertzforge_mimo_sine_law_math::linalg::mat_vec(&h, n, m, &d, &mut c);
        (0..n)
            .map(|k| (20.0 * (t.controls()[k].norm() / c[k].norm()).log10()).abs())
            .collect()
    };
    let mut v: Vec<f64> = r
        .ticks
        .iter()
        .filter(|t| !t.settling && t.miss.is_none())
        .flat_map(|t| {
            if tall {
                reach(t)
            } else {
                t.errors_db().iter().map(|e| e.abs()).collect::<Vec<_>>()
            }
        })
        .collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (med, worst) = (v[v.len() / 2], *v.last().unwrap());
    println!(
        "{} {:?}: outcome {:?}, median {med:.3} dB, worst {worst:.3} dB",
        s.name(),
        r.service.setup().test_type(),
        r.outcome
    );
    (r.outcome, med, worst)
}

fn tall(s: SineStructure) -> bool {
    s.n_controls() > s.dims().1
}

fn dwell_case(s: SineStructure) {
    let (o, _, worst) = case(s, dwell(96.0, 6.0), 10.0);
    assert_eq!(o, Some(Outcome::Completed));
    assert!(worst < if tall(s) { 0.5 } else { 0.25 }, "{worst}");
}

fn stepped_case(s: SineStructure) {
    let (o, _, worst) = case(s, stepped(), 60.0);
    assert_eq!(o, Some(Outcome::Completed));
    assert!(worst < 1.5, "{worst}");
}

fn sweep_case(s: SineStructure) {
    let (o, med, worst) = case(s, sweep((30.0, 300.0)), 250.0);
    assert_eq!(o, Some(Outcome::Completed));
    let tol = if tall(s) { 4.0 } else { 3.0 };
    assert!(worst < tol && med < 0.5, "median {med}, worst {worst}");
}

#[test]
fn mimo_sine_matrix_dwell_on_the_linear_4x4() {
    dwell_case(SineStructure::Linear4x4);
}

#[test]
fn mimo_sine_matrix_stepped_on_the_linear_4x4() {
    stepped_case(SineStructure::Linear4x4);
}

#[test]
fn mimo_sine_matrix_sweep_on_the_linear_4x4() {
    sweep_case(SineStructure::Linear4x4);
}

#[test]
fn mimo_sine_matrix_dwell_on_the_tall_6x3() {
    dwell_case(SineStructure::Tall6x3);
}

#[test]
fn mimo_sine_matrix_stepped_on_the_tall_6x3() {
    stepped_case(SineStructure::Tall6x3);
}

#[test]
fn mimo_sine_matrix_sweep_on_the_tall_6x3() {
    sweep_case(SineStructure::Tall6x3);
}

#[test]
fn mimo_sine_matrix_dwell_on_the_wide_3x6() {
    dwell_case(SineStructure::Wide3x6);
}

#[test]
fn mimo_sine_matrix_stepped_on_the_wide_3x6() {
    stepped_case(SineStructure::Wide3x6);
}

#[test]
fn mimo_sine_matrix_sweep_on_the_wide_3x6() {
    sweep_case(SineStructure::Wide3x6);
}

#[test]
fn mimo_sine_matrix_sweep_on_the_mildly_softening_4x4() {
    sweep_case(SineStructure::SofteningMild4x4);
}
