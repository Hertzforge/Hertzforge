//! The closed loop on a plant identified through noise, the reduced form of a
//! 48 x 12 lightly damped plate: more controls than drives, a reference the
//! plant cannot reach, and an estimate whose weak singular values sit on the
//! identification's noise floor, above the default truncation, in directions
//! the true plant barely answers in.
//!
//! - The default law and the buzz law keep the drive bounded over 60 loops and
//!   the response error does not grow (before the answer hold and the command
//!   bound, the drive rose from 2.6e4 to 3e9 and the worst error from 17 to
//!   65 dB).
//! - On the exact estimate the same run is unchanged by those rules.

// Index loops over lines and channels mirror the matrix algebra they check.
#![allow(clippy::needless_range_loop)]

mod common;

use common::*;
use hertzforge_mimo_laws::{BuiltinMimoLaw, BuzzMatchedLaw, PseudoInverseLaw};
use hertzforge_sdm::law::MimoControlLaw;
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

const LINES: usize = 32;
const BAND: (usize, usize) = (4, 28);
const LOOPS: usize = 60;
/// The response's noise floor on every control, PSD.
const FLOOR: f64 = 1e-6;

/// A 24 x 6 plant with four strong directions and two the structure barely
/// answers in (1e-4), and its estimate with complex noise of size `sigma` on
/// every element: at 1e-3 the estimate's two weak singular values are about
/// 3e-3 of the largest, above the default truncation of 1e-3.
fn plant_and_estimate(sigma: f64) -> (MatrixStack, MatrixStack) {
    let plant = plant_with_singular_values(11, 24, 6, LINES, &[1.0, 0.5, 0.25, 0.1, 1e-4, 1e-4]);
    let mut rng = Rng::new(77);
    let mut frf = plant.clone();
    for v in frf.data_mut() {
        *v += rng.cnormal() * sigma;
    }
    (plant, frf)
}

/// A random full-rank reference on 24 controls: the plant cannot reach it.
fn unreachable_reference(nc: usize) -> HermitianStack {
    let mut rng = Rng::new(5);
    let mut g = HermitianStack::zeros(LINES, nc);
    for line in BAND.0..BAND.1 {
        let x = random_psd(&mut rng, nc, 1e-2);
        for r in 0..nc {
            for c in r..nc {
                g.set(line, r, c, x[r * nc + c]);
            }
        }
    }
    g
}

fn drive_power(g: &HermitianStack) -> f64 {
    (0..g.lines()).map(|l| g.trace(l)).sum()
}

/// The drive power and the worst error of every loop, the response measured
/// through the true plant with the noise floor added.
fn run(law: &mut dyn MimoControlLaw, s: &Synthetic) -> Vec<(f64, f64)> {
    let needs = law.needs().to_vec();
    let mut response = s.sysid_response.clone();
    let mut last = s.sysid_drive.clone();
    let csd = cross(&s.plant, &last);
    let mut out = Vec::new();
    for l in 0..=LOOPS as u64 {
        let inputs = s.inputs(&response, &last, &csd, l, &needs);
        let o = if l == 0 {
            law.seed(&inputs)
        } else {
            law.iterate(&inputs)
        }
        .unwrap_or_else(|e| panic!("loop {l}: {e}"));
        last = o.drive;
        response = s.respond(&last, l + 1);
        for line in 0..LINES {
            for c in 0..s.setup.controls {
                let v = response.get(line, c, c);
                response.set(line, c, c, v + C64::new(FLOOR, 0.0));
            }
        }
        out.push((drive_power(&last), worst_db(s, &last)));
    }
    out
}

fn laws(s: &Synthetic) -> Vec<(&'static str, Box<dyn MimoControlLaw>)> {
    vec![
        (
            "pinv_closed",
            Box::new(PseudoInverseLaw::build(&s.setup, &[]).expect("law")),
        ),
        (
            "buzz_match",
            Box::new(BuzzMatchedLaw::build(&s.setup, &[]).expect("law")),
        ),
    ]
}

#[test]
fn the_closed_loop_does_not_wind_up_in_directions_the_plant_does_not_answer() {
    let (plant, frf) = plant_and_estimate(1e-3);
    let s = run_on(3, plant, frf, unreachable_reference(24), BAND, 0.01);
    for (name, mut law) in laws(&s) {
        let trace = run(law.as_mut(), &s);
        let (seed_power, _) = trace[0];
        let (_, first_err) = trace[1];
        let peak = trace.iter().map(|t| t.0).fold(0.0f64, f64::max);
        let (_, last_err) = *trace.last().expect("loops");
        eprintln!(
            "{name}: drive {seed_power:.3e} at the seed, peak {peak:.3e}; worst error {first_err:.2} dB at loop 1, {last_err:.2} dB at loop {LOOPS}"
        );
        assert!(
            peak <= 10.0 * seed_power,
            "{name}: the drive rose from {seed_power:.3e} to {peak:.3e}"
        );
        assert!(
            last_err <= first_err + 1.0,
            "{name}: the worst error grew from {first_err:.2} to {last_err:.2} dB"
        );
    }
}

#[test]
fn on_the_exact_estimate_the_rules_change_nothing_the_loop_needs() {
    let (plant, frf) = plant_and_estimate(0.0);
    let s = run_on(3, plant, frf, unreachable_reference(24), BAND, 0.01);
    for (name, mut law) in laws(&s) {
        let trace = run(law.as_mut(), &s);
        let (p0, e0) = trace[0];
        let (p, e) = *trace.last().expect("loops");
        assert!(
            (p / p0 - 1.0).abs() < 0.05 && (e - e0).abs() < 0.5,
            "{name}: drive {p0:.3e} to {p:.3e}, worst error {e0:.2} to {e:.2} dB"
        );
    }
}
