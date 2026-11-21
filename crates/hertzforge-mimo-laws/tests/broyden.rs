//! N. Hunter's Broyden congruence law against his note, "The Broyden
//! Congruence Law, Step by Step" (17 November 2026), and against the crate's
//! simulated structures.
//!
//! 1. The note's worked example, step by step (its sections 1.4, 3 and 4):
//!    every printed figure to half a unit of its last digit.
//! 2. The note's fourteen-cycle table, learning on and off, to 0.01 dB.
//! 3. The note's complex example and its phase steering table.
//! 4. Properties: the congruence keeps the coherence between drives, the
//!    model sensitivity's rows sum to 2 and 0, learning off, the step limit,
//!    the ceiling, the state kept, Hermitian positive semi-definite drives on
//!    all three shapes.
//! 5. The resonance schedule.
//! 6. The closed loop on the crate's simulated structures, learning on and
//!    off.
//! 7. Guards.

#![allow(clippy::needless_range_loop)]

mod common;

use common::*;
use hertzforge_mimo_laws::{
    resonance_gains, BroydenCongruenceLaw, BuiltinMimoLaw, LineRecord, ParamVal,
};
use hertzforge_sdm::law::{MimoControlLaw, MimoInput, MimoLoopInputs, MimoSetup, Tolerances};
use hertzforge_sdm::linalg::{self, Regularization};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

// -------------------------------------------------------------------------
// The note's example: one line, three accelerometers, two shakers
// -------------------------------------------------------------------------

fn c(re: f64) -> C64 {
    C64::new(re, 0.0)
}

fn polar(m: f64, deg: f64) -> C64 {
    C64::from_polar(m, deg.to_radians())
}

/// The note's real example (its section 1.4): H = [[1.0, 0.4], [0.5, 1.0],
/// [0.8, -0.6]].
fn note_h() -> Vec<C64> {
    vec![c(1.0), c(0.4), c(0.5), c(1.0), c(0.8), c(-0.6)]
}

/// The note's complex example (its section 5.3).
fn note_h_complex() -> Vec<C64> {
    vec![
        c(1.0),
        polar(0.4, 60.0),
        c(0.5),
        polar(1.0, -40.0),
        c(0.8),
        polar(0.6, 150.0),
    ]
}

/// One line; the band is that line.
fn one_line(h: &[C64], nc: usize, nd: usize) -> MatrixStack {
    MatrixStack::from_vec(1, nc, nd, h.to_vec()).expect("one line")
}

fn identity_reference(nc: usize) -> HermitianStack {
    let mut r = HermitianStack::zeros(1, nc);
    for i in 0..nc {
        r.set(0, i, i, c(1.0));
    }
    r
}

fn one_line_setup(nc: usize, nd: usize, reference: HermitianStack) -> MimoSetup {
    MimoSetup {
        controls: nc,
        drives: nd,
        lines: 1,
        df_hz: 1.0,
        band: (0, 1),
        reference,
        tolerances: Tolerances::none(1, nc),
    }
}

/// The note's example structure: the model, the true system and a noise
/// floor added to each control's measured autospectrum.
struct Toy {
    setup: MimoSetup,
    model: MatrixStack,
    truth: MatrixStack,
    floor: Vec<f64>,
    /// The number of averages: the response's degrees of freedom are twice it.
    averages: f64,
}

impl Toy {
    /// The fourteen-cycle example (note 4.14): the true system is the model
    /// with its third row 30 percent weaker, control 2 has a noise floor of
    /// 0.15, 30 averages.
    fn fourteen_cycle() -> Toy {
        let model = note_h();
        let mut truth = model.clone();
        for v in &mut truth[4..6] {
            *v *= 0.7;
        }
        Toy {
            setup: one_line_setup(3, 2, identity_reference(3)),
            model: one_line(&model, 3, 2),
            truth: one_line(&truth, 3, 2),
            floor: vec![0.0, 0.15, 0.0],
            averages: 30.0,
        }
    }

    /// The complex example, measured exactly (note 5.5).
    fn complex_exact() -> Toy {
        let h = note_h_complex();
        Toy {
            setup: one_line_setup(3, 2, identity_reference(3)),
            model: one_line(&h, 3, 2),
            truth: one_line(&h, 3, 2),
            floor: vec![0.0; 3],
            averages: 30.0,
        }
    }

    /// The measured response to `drive` and each control's multiple
    /// coherence, its signal over its signal plus the floor.
    fn measure(&self, drive: &HermitianStack) -> (HermitianStack, Vec<f64>) {
        let mut y = congruence(&self.truth, drive);
        let mut coh = Vec::new();
        for m in 0..self.setup.controls {
            let s = y.get(0, m, m).re;
            y.set(0, m, m, c(s + self.floor[m]));
            coh.push(s / (s + self.floor[m]));
        }
        (y, coh)
    }

    /// The error of each control in dB after `drive`, and their rms.
    fn errors(&self, drive: &HermitianStack) -> (Vec<f64>, f64) {
        let (y, _) = self.measure(drive);
        let e: Vec<f64> = (0..self.setup.controls)
            .map(|m| 10.0 * (y.get(0, m, m).re / self.setup.reference.get(0, m, m).re).log10())
            .collect();
        let rms = (e.iter().map(|v| v * v).sum::<f64>() / e.len() as f64).sqrt();
        (e, rms)
    }

    /// The first drive and `cycles` feedback cycles. Returns each drive (the
    /// first drive first) and each call's record of the line.
    fn run(
        &self,
        law: &mut BroydenCongruenceLaw,
        cycles: usize,
    ) -> (Vec<HermitianStack>, Vec<LineRecord>) {
        law.record_line(Some(0));
        let (mut response, mut coh) = (identity_reference(self.setup.controls), vec![1.0; 3]);
        let mut drives = Vec::new();
        let mut records = Vec::new();
        let mut last = HermitianStack::zeros(1, self.setup.drives);
        for k in 0..=cycles as u64 {
            let inputs = MimoLoopInputs {
                reference: &self.setup.reference,
                tolerances: &self.setup.tolerances,
                response: &response,
                response_dof: 2.0 * self.averages,
                frf: &self.model,
                multiple_coherence: &coh,
                df_hz: 1.0,
                band: (0, 1),
                loop_index: k,
                last_drive: Some(&last),
                response_drive_csd: None,
                sysid_response: None,
                sysid_drive: None,
                sysid_noise: None,
            };
            let out = if k == 0 {
                law.seed(&inputs)
            } else {
                law.iterate(&inputs)
            }
            .unwrap_or_else(|e| panic!("cycle {k}: {e}"));
            records.push(law.line_record().expect("recorded").clone());
            last = out.drive.clone();
            (response, coh) = self.measure(&last);
            drives.push(out.drive);
        }
        (drives, records)
    }
}

fn law_with(setup: &MimoSetup, given: &[(&str, ParamVal)]) -> BroydenCongruenceLaw {
    BroydenCongruenceLaw::with_params(setup, given).expect("builds")
}

/// The note's settings for its worked example and its table: the first
/// drive capped 9 dB down, and its Part 4 run without phase steering.
/// A key given twice resolves to its first value, so `extra` comes first.
fn note_part4(extra: &[(&'static str, ParamVal)]) -> Vec<(&'static str, ParamVal)> {
    let mut v = extra.to_vec();
    v.extend_from_slice(&[
        ("start_cap", ParamVal::Toggle(true)),
        ("start_cap_db", ParamVal::Number(-9.0)),
        ("phase_steering", ParamVal::Toggle(false)),
    ]);
    v
}

/// `|a - b| <= tol` element by element, named.
fn near(name: &str, got: &[f64], want: &[f64], tol: f64) {
    assert_eq!(got.len(), want.len(), "{name}: {got:?} against {want:?}");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert!(
            (g - w).abs() <= tol,
            "{name}[{i}]: {g} against the note's {w} (tolerance {tol}); all {got:?}"
        );
    }
}

fn re(v: &[C64]) -> Vec<f64> {
    v.iter().map(|z| z.re).collect()
}

fn coherence01(x: &[C64]) -> f64 {
    x[1].norm() / (x[0].re * x[3].re).sqrt()
}

// -------------------------------------------------------------------------
// 1. The worked example, step by step
// -------------------------------------------------------------------------

#[test]
fn the_first_drive_is_the_notes() {
    let toy = Toy::fourteen_cycle();
    // The note's pseudo-inverse (its 3.1), with the crate's truncated inverse
    // at the note's cutoff.
    let (plus, _) =
        linalg::pinv(&toy.model, Regularization::Truncated { rcond: 1e-3 }).expect("pinv");
    near(
        "H+",
        &re(plus.line(0)),
        &[0.5014, 0.1261, 0.5444, 0.1246, 0.6231, -0.5452],
        5e-5,
    );
    let mut law = law_with(&toy.setup, &note_part4(&[]));
    let (drives, records) = toy.run(&mut law, 0);
    let r = &records[0];
    near(
        "X_raw",
        &re(&r.raw),
        &[0.5637, -0.1558, -0.1558, 0.7009],
        5e-5,
    );
    near("predicted", &r.predicted, &[0.551, 0.686, 0.763], 5e-4);
    let sum: f64 = r.predicted.iter().sum();
    assert!(
        (sum - 2.0).abs() < 1e-12,
        "the predicted responses sum to {sum}, not 2"
    );
    near("s", &[r.start_scale], &[0.1888], 5e-5);
    let x0 = full(&drives[0], 0);
    near("X(0)", &re(&x0), &[0.1065, -0.0294, -0.0294, 0.1324], 5e-5);
    // "25% coherent" (3.2) and 0.248 (4.11): the same figure, 0.2478.
    near("coherence of X(0)", &[coherence01(&x0)], &[0.248], 5e-4);
    let y = congruence(&toy.model, &drives[0]);
    near(
        "model responses of X(0)",
        &(0..3).map(|m| y.get(0, m, m).re).collect::<Vec<_>>(),
        &[0.1041, 0.1296, 0.1440],
        5e-5,
    );

    // Off by default: the first drive at full level, the inverse itself.
    let mut full_level = law_with(&toy.setup, &[("phase_steering", ParamVal::Toggle(false))]);
    let (drives, records) = toy.run(&mut full_level, 0);
    assert_eq!(records[0].start_scale, 1.0);
    near(
        "full-level X(0)",
        &re(&full(&drives[0], 0)),
        &re(&records[0].raw),
        1e-15,
    );
}

#[test]
fn the_first_feedback_cycle_is_the_notes() {
    let toy = Toy::fourteen_cycle();
    let mut law = law_with(&toy.setup, &note_part4(&[]));
    let (drives, records) = toy.run(&mut law, 1);
    let r = &records[1];
    assert_eq!(r.cycle, 1);
    assert!(!r.base_played);
    // 4.2: the measured responses: the model's for controls 1 and 3 (row 3
    // 30 percent weaker: power 0.49), plus the floor on control 2.
    near("measured", &r.measured, &[0.1041, 0.2796, 0.0706], 5e-5);
    // 4.3: the shares and the model sensitivity.
    near("a_11, a_12", &re(&r.shares[0..2]), &[0.0947, 0.0094], 5e-5);
    near(
        "S_model",
        &r.s_model,
        &[1.819, 0.181, 0.184, 1.816, 1.142, 0.858],
        5e-4,
    );
    // 4.4: nothing to learn on the first cycle; the sensitivity is the model's.
    assert!(!r.updated);
    assert_eq!(r.s_hat, r.s_model);
    // 4.6: coherence weights, the signal over the signal plus the floor.
    near("weights", &r.weights, &[1.0, 0.4634, 1.0], 5e-5);
    // 4.7: c = 11.008 / 9.854 = 1.117.
    let (mut num, mut den) = (0.0, 0.0);
    for m in 0..3 {
        let rs = r.s_hat[m * 2] + r.s_hat[m * 2 + 1];
        num += r.weights[m] * rs * r.eps[m];
        den += r.weights[m] * rs * rs;
    }
    // Unrounded, the numerator is 11.00854, on the edge of the note's
    // 11.008; it is held to one unit of the last figure, not half. Its
    // quotient, 1.117, is the note's to half a unit.
    near("level split numerator", &[num], &[11.008], 1e-3);
    near("level split", &[den, r.level], &[9.854, 1.117], 5e-4);
    near("eps'", &r.eps_split, &[0.028, -0.960, 0.417], 5e-4);
    // 4.8: the ridge solve.
    near("S^T W S", &r.normal, &[4.630, 1.463, 1.463, 2.297], 5e-4);
    near("S^T W eps'", &r.rhs, &[0.445, -0.445], 5e-4);
    near("lambda", &[r.lambda], &[0.346], 5e-4);
    near("z", &r.z, &[0.166, -0.260], 5e-4);
    near("u = z + c", &r.u_solved, &[1.283, 0.857], 5e-4);
    // 4.9: K_i = 0.5, one line so no resonance cut: u = (0.642, 0.428).
    assert_eq!(r.resonance_gain, 1.0);
    let half: Vec<f64> = r.u_solved.iter().map(|v| 0.5 * v).collect();
    near("K_i u", &half, &[0.642, 0.428], 5e-4);
    // 4.10: the three predictions, the ratio, and the limited step: exactly
    // 3.0 dB on drive 1, 2.0 dB on drive 2.
    near("predictions", &r.predictions, &[1.245, 1.256, 1.283], 5e-4);
    near("ratio", &[r.ratio], &[1.858], 5e-4);
    near("u", &r.u, &[0.3454, 0.2306], 5e-5);
    let db: Vec<f64> =
        r.u.iter()
            .map(|v| 20.0 * v / std::f64::consts::LN_10)
            .collect();
    near("u in dB", &db, &[3.0, 2.0], 5e-2);
    assert!(
        (db[0] - 3.0).abs() < 1e-9,
        "the drive prediction sets the limit exactly: {db:?}"
    );
    // 4.11: the congruence.
    let d: Vec<f64> = r.u.iter().map(|v| v.exp()).collect();
    near("D", &d, &[1.4125, 1.2594], 5e-5);
    near(
        "X_new",
        &re(&r.applied),
        &[0.2124, -0.0523, -0.0523, 0.2099],
        5e-5,
    );
    near(
        "coherence after",
        &[coherence01(&r.applied)],
        &[0.248],
        5e-4,
    );
    // 4.12: the ceiling: nothing happens.
    near("y_exp", &r.expected, &[0.203, 0.453, 0.128], 5e-4);
    near("o", &[r.overshoot], &[0.029], 5e-4);
    near("limit", &[10f64.powf(1.2)], &[15.85], 5e-3);
    // The ceiling, floor and cap did nothing (the drive is stored as its
    // upper triangle, so the lower one may differ in the last bit).
    near("drive", &re(&r.drive), &re(&r.applied), 1e-16);
    near("returned", &re(&full(&drives[1], 0)), &re(&r.drive), 1e-16);
}

#[test]
fn the_second_cycle_update_is_the_notes() {
    let toy = Toy::fourteen_cycle();
    let mut law = law_with(&toy.setup, &note_part4(&[]));
    let (_, records) = toy.run(&mut law, 2);
    let r = &records[2];
    // 4.4.2, 4.4.3: the changes.
    near("du", &r.du, &[0.3454, 0.2306], 5e-5);
    near("dy", &r.dy, &[0.6734, 0.2548, 0.5974], 5e-5);
    // 4.4.4: the prediction and the innovation.
    near("p", &r.prediction, &[0.670, 0.482, 0.592], 5e-4);
    let innovation: Vec<f64> = (0..3).map(|m| r.dy[m] - r.prediction[m]).collect();
    // Unrounded, row 2's innovation is -0.22748, on the edge of the note's
    // -0.228; it is held to one unit of the last figure, not half.
    near("innovation", &innovation, &[0.003, -0.228, 0.005], 1e-3);
    // 4.4.5: the trust weights with 30 averages.
    near("w", &r.trust, &[0.871, 0.777, 0.840], 5e-4);
    assert!(r.updated);
    // 4.4.6: row 2's multiple, 0.6 x 0.777 x (-0.228) / 0.1725 = -0.615. The
    // note's own rounded factors give -0.616; the unrounded product is
    // -0.6151, the note's figure.
    let dudu: f64 = r.du.iter().map(|v| v * v).sum();
    near("du^T du", &[dudu], &[0.1725], 5e-5);
    let mult = 0.6 * r.trust[1] * innovation[1] / dudu;
    near("row 2 multiple", &[mult], &[-0.615], 5e-4);
    near("row 2 updated", &r.s_updated[2..4], &[-0.029, 1.674], 5e-4);
    let sums: Vec<f64> = (0..3)
        .map(|m| r.s_updated[m * 2] + r.s_updated[m * 2 + 1])
        .collect();
    near("row sums", &sums, &[2.006, 1.646, 2.009], 5e-4);
    // 4.4.7: rows 1 and 3 reduced by 0.003 and 0.004 per entry; row 2 kept.
    let cut: Vec<f64> = (0..3)
        .map(|m| r.s_updated[m * 2] - r.s_corrected[m * 2])
        .collect();
    near("row-sum correction", &cut, &[0.003, 0.0, 0.004], 5e-4);
    // 4.5: the anchor toward the model of the current drive.
    near("model row 2", &r.s_model[2..4], &[0.256, 1.744], 5e-4);
    near("anchored row 2", &r.s_hat[2..4], &[-0.014, 1.678], 5e-4);
}

// -------------------------------------------------------------------------
// 2. The fourteen-cycle table
// -------------------------------------------------------------------------

/// The note's table (4.14): cycle, the three errors with learning on, the rms
/// with learning on and off.
const TABLE: [(usize, [f64; 3], f64, f64); 8] = [
    (1, [-6.90, -4.43, -8.92], 7.00, 7.00),
    (2, [-3.92, -2.69, -6.07], 4.45, 4.59),
    (3, [-1.53, -0.90, -3.70], 2.37, 2.55),
    (4, [-0.32, 0.02, -2.54], 1.48, 1.62),
    (5, [0.30, 0.44, -1.97], 1.18, 1.25),
    (6, [0.61, 0.62, -1.70], 1.10, 1.13),
    (8, [0.84, 0.73, -1.51], 1.08, 1.08),
    (14, [0.92, 0.71, -1.46], 1.08, 1.08),
];

fn table_residual(phase: bool) -> f64 {
    let toy = Toy::fourteen_cycle();
    let p = [("phase_steering", ParamVal::Toggle(phase))];
    let mut on = law_with(&toy.setup, &note_part4(&p));
    let mut off = law_with(
        &toy.setup,
        &note_part4(&[p[0].clone(), ("broyden_gain", ParamVal::Number(0.0))]),
    );
    let (on_drives, _) = toy.run(&mut on, 14);
    let (off_drives, off_records) = toy.run(&mut off, 14);
    assert!(
        off_records.iter().all(|r| !r.updated),
        "learning off learned"
    );
    let mut worst: f64 = 0.0;
    for (cycle, errors, rms_on, rms_off) in TABLE {
        let (e, r_on) = toy.errors(&on_drives[cycle]);
        let (_, r_off) = toy.errors(&off_drives[cycle]);
        println!(
            "phase {phase} cycle {cycle:2}: {:+.3} {:+.3} {:+.3}  rms on {r_on:.3} off {r_off:.3}   \
             note {:+.2} {:+.2} {:+.2} {rms_on:.2} {rms_off:.2}",
            e[0], e[1], e[2], errors[0], errors[1], errors[2]
        );
        for m in 0..3 {
            worst = worst.max((e[m] - errors[m]).abs());
        }
        worst = worst
            .max((r_on - rms_on).abs())
            .max((r_off - rms_off).abs());
    }
    worst
}

#[test]
fn the_fourteen_cycle_table_is_reproduced_learning_on_and_off() {
    let worst = table_residual(false);
    println!("worst difference from the note's table: {worst:.4} dB");
    assert!(worst <= 0.01, "{worst} dB");
    // With phase steering on, the ridge counts 2N unknowns, and the table is
    // not the note's: it was run without (Choice 2).
    let with_phase = table_residual(true);
    println!("with phase steering on: {with_phase:.4} dB");
    assert!(with_phase > 0.05, "{with_phase}");
}

// -------------------------------------------------------------------------
// 3. The complex example and the phase table
// -------------------------------------------------------------------------

#[test]
fn the_complex_example_is_the_notes() {
    let toy = Toy::complex_exact();
    let mut law = law_with(&toy.setup, &[("start_cap", ParamVal::Toggle(true))]);
    let (drives, records) = toy.run(&mut law, 1);
    let r = &records[1];
    // 5.3: magnitude columns, then phase columns.
    let s = &r.s_model;
    let mag: Vec<f64> = (0..3).flat_map(|m| s[m * 4..m * 4 + 2].to_vec()).collect();
    let ph: Vec<f64> = (0..3)
        .flat_map(|m| s[m * 4 + 2..m * 4 + 4].to_vec())
        .collect();
    near(
        "magnitude",
        &mag,
        &[1.775, 0.225, 0.347, 1.653, 1.176, 0.824],
        5e-4,
    );
    near(
        "phase",
        &ph,
        &[-0.006, 0.006, 0.134, -0.134, -0.181, 0.181],
        5e-4,
    );
    // Rotating shaker 2 by 0.1 rad: the linear prediction and the exact change.
    let x0 = &drives[0];
    let lin: Vec<f64> = (0..3).map(|m| 0.1 * s[m * 4 + 3]).collect();
    near("linear", &lin, &[0.0006, -0.0134, 0.0181], 5e-5);
    let rot = C64::from_polar(1.0, 0.1);
    let mut x1 = x0.clone();
    x1.set(0, 0, 1, x0.get(0, 0, 1) * rot.conj());
    let (y0, y1) = (congruence(&toy.model, x0), congruence(&toy.model, &x1));
    let exact: Vec<f64> = (0..3)
        .map(|m| (y1.get(0, m, m).re / y0.get(0, m, m).re).ln())
        .collect();
    near("exact", &exact, &[0.0014, -0.0136, 0.0178], 5e-5);
    let deg = |x: &HermitianStack| x.get(0, 0, 1).arg().to_degrees();
    near(
        "phase between shakers",
        &[deg(x0), deg(&x1)],
        &[-122.3, -128.0],
        5e-2,
    );
    let x0f = full(x0, 0);
    let x1f = full(&x1, 0);
    near(
        "coherence",
        &[coherence01(&x0f), coherence01(&x1f)],
        &[0.185, 0.185],
        5e-4,
    );
}

#[test]
fn the_phase_table_is_reproduced() {
    // 5.5: the complex example, exact measurements; phase steering on and
    // off. Rms error in dB at cycles 5, 10, 20, 60.
    const WANT: [(usize, f64, f64, f64); 4] = [
        (5, 0.45, 0.47, 5e-3),
        (10, 0.06, 0.18, 5e-3),
        (20, 0.02, 0.18, 5e-3),
        (60, 0.001, 0.18, 5e-4),
    ];
    let toy = Toy::complex_exact();
    let cap = ("start_cap", ParamVal::Toggle(true));
    let mut on = law_with(&toy.setup, std::slice::from_ref(&cap));
    let mut off = law_with(
        &toy.setup,
        &[cap, ("phase_steering", ParamVal::Toggle(false))],
    );
    let (on_d, _) = toy.run(&mut on, 60);
    let (off_d, _) = toy.run(&mut off, 60);
    for (cycle, want_on, want_off, tol) in WANT {
        let (_, r_on) = toy.errors(&on_d[cycle]);
        let (_, r_off) = toy.errors(&off_d[cycle]);
        println!("cycle {cycle}: on {r_on:.4} (note {want_on}), off {r_off:.4} (note {want_off})");
        assert!((r_on - want_on).abs() <= tol, "cycle {cycle} on: {r_on}");
        assert!(
            (r_off - want_off).abs() <= 5e-3,
            "cycle {cycle} off: {r_off}"
        );
    }
}

// -------------------------------------------------------------------------
// 4. Properties
// -------------------------------------------------------------------------

const LINES: usize = 64;
const BAND: (usize, usize) = (8, 56);

/// The crate's structures with a reachable reference and a 5 percent error
/// in the estimate.
fn structures() -> Vec<(String, Synthetic)> {
    let mut v = Vec::new();
    for (i, (nc, nd)) in [(3, 2), (6, 4), (12, 6), (4, 4), (4, 8)]
        .into_iter()
        .enumerate()
    {
        let seed = 100 + i as u64;
        let plant = well_conditioned(seed, nc, nd, LINES);
        let reference = reachable_reference(&plant, seed, BAND);
        let frf = perturbed(&plant, seed, 0.05);
        v.push((
            format!("{nc} x {nd}"),
            run_on(seed, plant, frf, reference, BAND, 0.01),
        ));
    }
    v
}

fn pair_coherence(x: &HermitianStack, line: usize, i: usize, j: usize) -> (f64, f64) {
    let v = x.get(line, i, j);
    let d = (x.get(line, i, i).re * x.get(line, j, j).re).sqrt();
    if d == 0.0 {
        (0.0, 0.0)
    } else {
        (v.norm() / d, v.arg())
    }
}

#[test]
fn the_congruence_keeps_the_coherence_between_drives() {
    // The step itself, base drive to applied drive, before the ceiling,
    // floor and cap (the cap may then change a pair; that is Choice 15).
    for (name, s) in structures() {
        let mut worst: f64 = 0.0;
        let mut moved: f64 = 0.0;
        for line in [BAND.0, 20, 37, BAND.1 - 1] {
            let mut law = law_with(&s.setup, &[]);
            law.record_line(Some(line));
            let needs = law.needs().to_vec();
            let mut response = s.sysid_response.clone();
            let mut last = s.sysid_drive.clone();
            let csd = cross(&s.plant, &last);
            for l in 0..=6u64 {
                let inputs = s.inputs(&response, &last, &csd, l, &needs);
                let out = if l == 0 {
                    law.seed(&inputs)
                } else {
                    law.iterate(&inputs)
                }
                .expect("call");
                if l > 0 {
                    let r = law.line_record().expect("recorded");
                    let n = s.setup.drives;
                    let (a, b) = (&r.base, &r.applied);
                    for i in 0..n {
                        moved = moved.max((b[i * n + i].re / a[i * n + i].re).ln().abs());
                        for j in (i + 1)..n {
                            let ca =
                                a[i * n + j].norm() / (a[i * n + i].re * a[j * n + j].re).sqrt();
                            let cb =
                                b[i * n + j].norm() / (b[i * n + i].re * b[j * n + j].re).sqrt();
                            worst = worst.max((ca - cb).abs());
                        }
                    }
                }
                last = out.drive;
                response = s.respond(&last, l + 1);
            }
        }
        println!("{name}: coherence moved at most {worst:.2e} while levels moved {moved:.3}");
        assert!(moved > 1e-3, "{name}: nothing moved");
        assert!(worst < 1e-12, "{name}: {worst}");
    }
}

#[test]
fn each_model_sensitivity_row_sums_to_two_and_its_phases_to_zero() {
    for (name, s) in structures() {
        let mut law = law_with(&s.setup, &[]);
        law.record_line(Some(20));
        let _ = closed_loop(&mut law, &s, 1);
        let r = law.line_record().expect("recorded");
        let (nc, nd) = (s.setup.controls, s.setup.drives);
        let p = 2 * nd;
        for m in 0..nc {
            let mag: f64 = r.s_model[m * p..m * p + nd].iter().sum();
            let ph: f64 = r.s_model[m * p + nd..(m + 1) * p].iter().sum();
            assert!((mag - 2.0).abs() < 1e-12, "{name} row {m}: {mag}");
            assert!(ph.abs() < 1e-12, "{name} row {m}: {ph}");
        }
        assert!(
            r.s_model.iter().skip(nd).step_by(1).any(|v| v.abs() > 1e-3),
            "{name}: a complex structure has phase sensitivity"
        );
    }
}

#[test]
fn with_learning_off_the_sensitivity_in_use_follows_the_model() {
    let s = &structures()[1].1;
    let line = 30;
    // Learning off, anchor 1: the model's own, every loop.
    let mut law = law_with(
        &s.setup,
        &[
            ("broyden_gain", ParamVal::Number(0.0)),
            ("model_anchor", ParamVal::Number(1.0)),
        ],
    );
    law.record_line(Some(line));
    let needs = law.needs().to_vec();
    let mut response = s.sysid_response.clone();
    let mut last = s.sysid_drive.clone();
    let csd = cross(&s.plant, &last);
    for l in 0..=6u64 {
        let inputs = s.inputs(&response, &last, &csd, l, &needs);
        let out = if l == 0 {
            law.seed(&inputs)
        } else {
            law.iterate(&inputs)
        }
        .expect("call");
        if l > 0 {
            let r = law.line_record().expect("recorded");
            assert_eq!(r.s_hat, r.s_model, "loop {l}");
        }
        last = out.drive;
        response = s.respond(&last, l + 1);
    }
    // Learning off, the note's anchor: each loop 5 percent of the way from
    // last loop's sensitivity to the current model's, and nothing learned.
    let mut law = law_with(&s.setup, &[("broyden_gain", ParamVal::Number(0.0))]);
    law.record_line(Some(line));
    let mut response = s.sysid_response.clone();
    let mut last = s.sysid_drive.clone();
    let mut before: Option<Vec<f64>> = None;
    for l in 0..=6u64 {
        let inputs = s.inputs(&response, &last, &csd, l, &needs);
        let out = if l == 0 {
            law.seed(&inputs)
        } else {
            law.iterate(&inputs)
        }
        .expect("call");
        if l > 0 {
            let r = law.line_record().expect("recorded");
            assert!(!r.updated);
            let want: Vec<f64> = match &before {
                None => r.s_model.clone(),
                Some(b) => b
                    .iter()
                    .zip(&r.s_model)
                    .map(|(h, m)| 0.95 * h + 0.05 * m)
                    .collect(),
            };
            let gap = want
                .iter()
                .zip(&r.s_hat)
                .fold(0.0f64, |w, (a, b)| w.max((a - b).abs()));
            assert!(gap < 1e-12, "loop {l}: {gap}");
            before = Some(r.s_hat.clone());
        }
        last = out.drive;
        response = s.respond(&last, l + 1);
    }
}

#[test]
fn the_step_limit_holds_each_of_its_three_predictions() {
    let mut cases = 0;
    let mut acted = 0;
    let mut phase_held = 0;
    for (name, s) in structures() {
        for (max_step_db, max_phase_deg) in [(0.1, 15.0), (3.0, 15.0), (3.0, 1.0)] {
            for line in [BAND.0, 20, 37, BAND.1 - 1] {
                let mut law = law_with(
                    &s.setup,
                    &[
                        ("max_step_db", ParamVal::Number(max_step_db)),
                        ("max_phase_step_deg", ParamVal::Number(max_phase_deg)),
                    ],
                );
                law.record_line(Some(line));
                let needs = law.needs().to_vec();
                let mut response = s.sysid_response.clone();
                let mut last = s.sysid_drive.clone();
                let csd = cross(&s.plant, &last);
                for l in 0..=4u64 {
                    let inputs = s.inputs(&response, &last, &csd, l, &needs);
                    let out = if l == 0 {
                        law.seed(&inputs)
                    } else {
                        law.iterate(&inputs)
                    }
                    .expect("call");
                    if l > 0 {
                        let r = law.line_record().expect("recorded");
                        let limit = max_step_db * std::f64::consts::LN_10 / 10.0;
                        let (nc, nd) = (s.setup.controls, s.setup.drives);
                        let p = r.u.len();
                        let h = s.frf.line(line);
                        let mut learned: f64 = 0.0;
                        for m in 0..nc {
                            if r.usable[m] {
                                let su: f64 = (0..p).map(|j| r.s_hat[m * p + j] * r.u[j]).sum();
                                learned = learned.max(su.abs());
                            }
                        }
                        let y = |x: &[C64], m: usize| -> f64 {
                            let mut s = 0.0;
                            for i in 0..nd {
                                for j in 0..nd {
                                    s += (h[m * nd + i] * x[i * nd + j] * h[m * nd + j].conj()).re;
                                }
                            }
                            s
                        };
                        let exact = (0..nc)
                            .map(|m| (y(&r.applied, m) / y(&r.base, m)).ln().abs())
                            .fold(0.0f64, f64::max);
                        let drive = 2.0 * r.u[..nd].iter().fold(0.0f64, |a, v| a.max(v.abs()));
                        let phase = r.u[nd..].iter().fold(0.0f64, |a, v| a.max(v.abs()));
                        let bar = limit * (1.0 + 1e-9);
                        assert!(
                            learned <= bar,
                            "{name} line {line} loop {l}: learned {learned}"
                        );
                        assert!(exact <= bar, "{name} line {line} loop {l}: exact {exact}");
                        assert!(drive <= bar, "{name} line {line} loop {l}: drive {drive}");
                        let max_phase = max_phase_deg.to_radians();
                        assert!(phase <= max_phase * (1.0 + 1e-9), "{name}: phase {phase}");
                        phase_held += (phase >= max_phase * (1.0 - 1e-9)) as usize;
                        // The phases have zero mean (Choice 5). This holds
                        // with or without the law's shift, which is redundant
                        // in exact arithmetic; it does not test the shift.
                        let mean: f64 = r.u[nd..].iter().sum::<f64>() / nd as f64;
                        assert!(mean.abs() < 1e-15, "{name}: phase mean {mean}");
                        cases += 1;
                        acted += (r.ratio > 1.0) as usize;
                    }
                    last = out.drive;
                    response = s.respond(&last, l + 1);
                }
            }
        }
    }
    println!(
        "step limit: {cases} steps checked, {acted} of them limited, {phase_held} by the phase"
    );
    assert!(acted > 10 && acted < cases, "{acted} of {cases}");
    assert!(phase_held > 0);
}

/// A structure 20 dB louder than its estimate: the first feedback step's
/// expected response is far above the ceiling.
fn too_loud() -> Synthetic {
    let s = &structures()[0].1;
    let mut loud = run_on(
        5,
        scaled(&s.plant, c(10.0)),
        s.plant.clone(),
        s.setup.reference.clone(),
        BAND,
        0.0,
    );
    loud.coherence = vec![1.0; LINES * 3];
    loud
}

#[test]
fn the_ceiling_acts_in_one_loop_and_the_state_kept_is_the_drive_after_it() {
    let s = too_loud();
    let line = 30;
    let mut law = law_with(&s.setup, &[("ceiling_db", ParamVal::Number(3.0))]);
    law.record_line(Some(line));
    let run_needs = law.needs().to_vec();
    let mut response = s.sysid_response.clone();
    let mut last = s.sysid_drive.clone();
    let csd = cross(&s.plant, &last);
    let mut outs = Vec::new();
    let mut recs = Vec::new();
    for l in 0..=2u64 {
        let inputs = s.inputs(&response, &last, &csd, l, &run_needs);
        let out = if l == 0 {
            law.seed(&inputs)
        } else {
            law.iterate(&inputs)
        }
        .expect("call");
        recs.push(law.line_record().expect("recorded").clone());
        last = out.drive.clone();
        response = s.respond(&last, l + 1);
        outs.push(out);
    }
    let r = &recs[1];
    // The ceiling acted on the first loop: the drive is the applied one over
    // o, and the largest expected response over its limit is now 1.
    assert!(r.overshoot > 1.5, "{}", r.overshoot);
    let factor = per_line_of(&outs[1], "ceiling_factor")[line];
    assert!((factor - 1.0 / r.overshoot).abs() < 1e-15);
    for (a, d) in r.applied.iter().zip(&r.drive) {
        assert!((a * factor - d).norm() <= 1e-15 * a.norm().max(1e-300));
    }
    let limit = 10f64.powf(0.3);
    let worst = (0..3)
        .filter(|&m| r.usable[m])
        .map(|m| r.expected[m] * factor / (s.setup.reference.get(line, m, m).re * limit))
        .fold(0.0f64, f64::max);
    assert!((worst - 1.0).abs() < 1e-12, "{worst}");
    // The state kept is that drive: the next loop builds on it and learns
    // from the change that went out.
    let r2 = &recs[2];
    assert_eq!(
        r2.base,
        full(&outs[1].drive, line),
        "the base is the drive returned"
    );
    let (a, b) = (outs[1].drive.diagonal(line), outs[0].drive.diagonal(line));
    let want: Vec<f64> = (0..2).map(|i| 0.5 * (a[i] / b[i]).ln()).collect();
    for i in 0..2 {
        assert!(
            (r2.du[i] - want[i]).abs() < 1e-12,
            "du {:?} against {want:?}",
            r2.du
        );
    }
}

fn per_line_of(out: &hertzforge_sdm::law::MimoLoopOutputs, key: &str) -> Vec<f64> {
    match out
        .diagnostics
        .iter()
        .find(|d| d.key == key)
        .map(|d| &d.data)
    {
        Some(hertzforge_sdm::law::DiagnosticData::PerLine(v)) => v.clone(),
        other => panic!("{key}: {other:?}"),
    }
}

fn scalar_of(out: &hertzforge_sdm::law::MimoLoopOutputs, key: &str) -> f64 {
    match out
        .diagnostics
        .iter()
        .find(|d| d.key == key)
        .map(|d| &d.data)
    {
        Some(hertzforge_sdm::law::DiagnosticData::Scalar(v)) => *v,
        other => panic!("{key}: {other:?}"),
    }
}

#[test]
fn every_drive_is_hermitian_psd_and_zero_out_of_band_on_all_three_shapes() {
    let mut cases = structures();
    let ill = plant_with_singular_values(7, 6, 4, LINES, &[1.0, 0.3, 0.1, 1e-5]);
    let gain = C64::from_polar(10f64.powf(1.5 / 20.0), 10f64.to_radians());
    let ill_ref = reachable_reference(&ill, 8, BAND);
    cases.push((
        "6 x 4 ill-conditioned".into(),
        run_on(8, ill.clone(), scaled(&ill, gain), ill_ref, BAND, 0.01),
    ));
    let unreach = plant_with_singular_values(41, 24, 6, LINES, &[1.0, 0.3, 0.1, 0.03, 0.01, 3e-3]);
    let mut s = Synthetic::with_plant(41, unreach.clone(), BAND, 0.01);
    s.frf = unreach;
    cases.push(("24 x 6 unreachable".into(), s));
    // A random reference: its pseudo-inverse drive has pairs above the cap.
    cases.push(("3 x 4 random".into(), Synthetic::random(9, 3, 4, LINES)));
    let mut capped = 0usize;
    let mut over: f64 = 0.0;
    for (name, s) in &cases {
        for given in [vec![], vec![("phase_steering", ParamVal::Toggle(false))]] {
            let mut law = law_with(&s.setup, &given);
            let run = closed_loop(&mut law, s, 20);
            for (l, d) in run.drives().enumerate() {
                let what = format!("{name} loop {l}");
                assert!(d.is_finite(), "{what}");
                for line in 0..LINES {
                    for r in 0..d.n() {
                        assert_eq!(d.get(line, r, r).im, 0.0, "{what}");
                        for c in (r + 1)..d.n() {
                            let (coh, _) = pair_coherence(d, line, r, c);
                            // Two drives: the shrink alone, exact. Three or
                            // more: the eigenvalue repair after it is not
                            // capped again (Choice 15), so a pair may end a
                            // little above.
                            let bar = if d.n() < 3 { 1e-9 } else { 1e-3 };
                            assert!(coh <= 0.95 + bar, "{what} line {line}: coherence {coh}");
                            over = over.max(coh - 0.95);
                            capped += (coh > 0.95 - 1e-9) as usize;
                        }
                    }
                    if line < BAND.0 || line >= BAND.1 {
                        assert!(
                            d.line(line).iter().all(|v| *v == C64::new(0.0, 0.0)),
                            "{what}"
                        );
                    }
                }
                assert!(worst_eigen_ratio(d) >= -1e-9, "{what}");
            }
        }
    }
    println!("drive pairs held at the coherence cap: {capped}; most above it after the repair: {over:.2e}");
    assert!(capped > 0, "the cap never acted");
}

// -------------------------------------------------------------------------
// 5. The resonance schedule
// -------------------------------------------------------------------------

/// A stack of `nb` lines, `nc` controls, one drive, row sizes from `size`.
fn rows(nb: usize, nc: usize, size: impl Fn(usize, usize) -> f64) -> MatrixStack {
    let mut h = MatrixStack::zeros(nb, nc, 1);
    for i in 0..nb {
        for k in 0..nc {
            h.set(i, k, 0, C64::new(0.0, size(i, k)));
        }
    }
    h
}

#[test]
fn the_resonance_schedule_cuts_a_peak_and_nothing_else() {
    let (nb, first) = (200, 40);
    let all = vec![true; nb * 2];
    let g = |h: &MatrixStack, spec: &[bool], s: f64, min: f64| {
        resonance_gains(h, first, spec, 1.0 / 3.0, s, min)
    };
    // A sharp peak in control 1's row, 10 times its neighbors at line 100.
    let peak = rows(nb, 2, |i, k| if k == 1 && i == 100 { 10.0 } else { 1.0 });
    let gains = g(&peak, &all, 0.5, 0.0);
    assert!(
        (gains[100] - 1.0 / (1.0 + 0.5 * 9.0)).abs() < 1e-15,
        "{}",
        gains[100]
    );
    assert!(gains.iter().enumerate().all(|(i, &v)| i == 100 || v == 1.0));
    // A notch is not cut.
    let notch = rows(nb, 2, |i, k| if k == 0 && i == 100 { 0.1 } else { 1.0 });
    assert!(g(&notch, &all, 0.5, 0.0).iter().all(|&v| v == 1.0));
    // A smooth trend, rising or falling, is not cut, at the band's edges too.
    for trend in [
        rows(nb, 2, |i, _| ((first + i) as f64).powi(2)),
        rows(nb, 2, |i, _| 1.0 / ((first + i) as f64).powi(3)),
        rows(nb, 2, |i, k| {
            (-0.02 * (first + i) as f64).exp() * (k + 1) as f64
        }),
    ] {
        let gains = g(&trend, &all, 0.5, 0.0);
        assert!(gains.iter().all(|&v| v == 1.0), "{gains:?}");
    }
    // The worst control is taken: peaks of 10 and 4 on one line.
    let two = rows(nb, 2, |i, k| if i == 100 { [4.0, 10.0][k] } else { 1.0 });
    assert!((g(&two, &all, 0.5, 0.0)[100] - 1.0 / 5.5).abs() < 1e-15);
    // A control with no specification on the line does not count.
    let mut spec = all.clone();
    spec[100 * 2 + 1] = false;
    assert!((g(&two, &spec, 0.5, 0.0)[100] - 1.0 / 2.5).abs() < 1e-15);
    // The smallest gain, and the schedule off at a sensitivity of zero.
    assert_eq!(g(&peak, &all, 0.5, 0.5)[100], 0.5);
    assert!(g(&peak, &all, 0.0, 0.0).iter().all(|&v| v == 1.0));
    // One line in the band: the ratio is one.
    let one = rows(1, 2, |_, _| 7.0);
    assert_eq!(
        resonance_gains(&one, 10, &[true, true], 1.0 / 3.0, 0.5, 0.0),
        vec![1.0]
    );
    // Near the bottom of the window, too few lines: no cut.
    let low = rows(20, 1, |i, _| if i == 2 { 10.0 } else { 1.0 });
    assert_eq!(
        resonance_gains(&low, 1, &[true; 20], 1.0 / 3.0, 0.5, 0.0)[2],
        1.0
    );
}

#[test]
fn the_law_reports_the_resonance_gain_it_used() {
    let s = &structures()[0].1;
    let mut law = law_with(&s.setup, &[]);
    law.record_line(Some(30));
    let run = closed_loop(&mut law, s, 1);
    let g = per_line_of(&run.outputs[1], "resonance_gain");
    let spec: Vec<bool> = (BAND.0..BAND.1)
        .flat_map(|l| (0..3).map(move |m| (l, m)))
        .map(|(l, m)| s.setup.reference.get(l, m, m).re > 0.0)
        .collect();
    let mut hb = MatrixStack::zeros(BAND.1 - BAND.0, 3, 2);
    for (i, l) in (BAND.0..BAND.1).enumerate() {
        hb.line_mut(i).copy_from_slice(s.frf.line(l));
    }
    let want = resonance_gains(&hb, BAND.0, &spec, 1.0 / 3.0, 0.5, 0.0);
    assert_eq!(&g[BAND.0..BAND.1], want.as_slice());
    assert_eq!(law.line_record().expect("recorded").resonance_gain, g[30]);
    assert!(g.iter().all(|&v| v > 0.0 && v <= 1.0));
    // The step is the solve times the convergence factor and that gain,
    // then the step limit's scale: on the line with the deepest cut.
    let line = (BAND.0..BAND.1)
        .min_by(|&a, &b| g[a].total_cmp(&g[b]))
        .expect("band");
    assert!(g[line] < 0.9, "{}", g[line]);
    let mut law = law_with(&s.setup, &[]);
    law.record_line(Some(line));
    let run = closed_loop(&mut law, s, 1);
    let r = law.line_record().expect("recorded");
    let scale = per_line_of(&run.outputs[1], "step_scale")[line];
    for (u, z) in r.u.iter().zip(&r.u_solved) {
        assert!((u - 0.5 * g[line] * scale * z).abs() <= 1e-15 * z.abs().max(1e-300));
    }
}

// -------------------------------------------------------------------------
// 6. The closed loop on the crate's structures
// -------------------------------------------------------------------------

#[test]
fn the_closed_loop_on_the_crates_structures_learning_on_and_off() {
    const LOOPS: usize = 30;
    let mut cases: Vec<(String, Synthetic)> = structures().into_iter().take(3).collect();
    let ill = plant_with_singular_values(7, 6, 4, LINES, &[1.0, 0.3, 0.1, 1e-5]);
    let gain = C64::from_polar(10f64.powf(1.5 / 20.0), 10f64.to_radians());
    let ill_ref = reachable_reference(&ill, 8, BAND);
    cases.push((
        "6 x 4 ill-conditioned".into(),
        run_on(8, ill.clone(), scaled(&ill, gain), ill_ref, BAND, 0.01),
    ));
    for (name, s) in &cases {
        for (label, given) in [
            ("learning on", vec![]),
            (
                "learning off",
                vec![("broyden_gain", ParamVal::Number(0.0))],
            ),
        ] {
            let mut law = law_with(&s.setup, &given);
            let run = closed_loop(&mut law, s, LOOPS);
            let seed_db = worst_db(s, &run.outputs[0].drive);
            let last_db = worst_db(s, run.last());
            let to = |db: f64| run.drives().position(|d| worst_db(s, d) <= db);
            println!(
                "{name}, {label}: first drive {seed_db:.2} dB, after {LOOPS} loops {last_db:.3} dB, \
                 within 1 dB from loop {:?}, 0.5 dB from {:?}, 0.1 dB from {:?}; updates {}",
                to(1.0),
                to(0.5),
                to(0.1),
                scalar_of(run.outputs.last().expect("ran"), "broyden_updates"),
            );
            assert!(
                last_db < 0.5,
                "{name}, {label}: {last_db} dB after {LOOPS} loops"
            );
        }
    }
}

// -------------------------------------------------------------------------
// 7. Guards
// -------------------------------------------------------------------------

/// One loop on `s` with the response altered by `edit`; the drive must be
/// finite, Hermitian PSD and zero out of band.
fn one_loop_with(
    s: &Synthetic,
    given: &[(&str, ParamVal)],
    edit: impl Fn(&mut HermitianStack, &mut Vec<f64>),
    line: usize,
) -> (hertzforge_sdm::law::MimoLoopOutputs, LineRecord) {
    let mut law = law_with(&s.setup, given);
    law.record_line(Some(line));
    let needs = law.needs().to_vec();
    let csd = cross(&s.plant, &s.sysid_drive);
    let inputs = s.inputs(&s.sysid_response, &s.sysid_drive, &csd, 0, &needs);
    let seed = law.seed(&inputs).expect("seed");
    let mut response = s.respond(&seed.drive, 1);
    let mut coh = s.coherence.clone();
    edit(&mut response, &mut coh);
    let mut inputs = s.inputs(&response, &seed.drive, &csd, 1, &needs);
    inputs.multiple_coherence = &coh;
    let out = law.iterate(&inputs).expect("a defined drive");
    assert!(out.drive.is_finite());
    assert!(worst_eigen_ratio(&out.drive) >= -1e-9);
    for l in (0..BAND.0).chain(BAND.1..LINES) {
        assert!(out.drive.line(l).iter().all(|v| *v == C64::new(0.0, 0.0)));
    }
    for d in &out.diagnostics {
        let ok = match &d.data {
            hertzforge_sdm::law::DiagnosticData::Scalar(v) => v.is_finite(),
            hertzforge_sdm::law::DiagnosticData::PerLine(v)
            | hertzforge_sdm::law::DiagnosticData::PerControlLine(v)
            | hertzforge_sdm::law::DiagnosticData::PerDriveLine(v) => {
                v.iter().all(|x| x.is_finite())
            }
        };
        assert!(ok, "{} is not finite", d.key);
    }
    (out, law.line_record().expect("recorded").clone())
}

#[test]
fn a_control_reading_zero_negative_or_not_a_number_is_left_out() {
    let s = &structures()[1].1;
    let line = 30;
    for bad in [0.0, -1e-3, f64::NAN] {
        let (_, r) = one_loop_with(
            s,
            &[],
            |y, _| {
                let v = y.get(line, 2, 2);
                y.set(line, 2, 2, C64::new(bad, v.im));
            },
            line,
        );
        assert!(!r.usable[2], "{bad}");
        assert!(r.usable.iter().enumerate().all(|(m, &u)| u || m == 2));
        assert_eq!(r.weights[2], 0.0);
        assert_eq!(r.eps[2], 0.0);
        assert!(
            r.u.iter().any(|v| *v != 0.0),
            "the other controls still step"
        );
    }
}

#[test]
fn the_ceiling_holds_a_control_left_out_to_its_model_response() {
    // Control 2's target on one line is 60 dB below what the drive for the
    // others gives it, and its reading is not a number, so it is left out of
    // the step; the ceiling still holds its model response to 20 dB above
    // its own limit (Choice 17).
    let mut s = structures()[1].1.clone_run();
    let line = 30;
    let t2 = s.setup.reference.get(line, 2, 2).re * 1e-6;
    for k in 0..6 {
        s.setup.reference.set(line, 2, k, C64::new(0.0, 0.0));
    }
    s.setup.reference.set(line, 2, 2, C64::new(t2, 0.0));
    let (out, r) = one_loop_with(
        &s,
        &[],
        |y, _| y.set(line, 2, 2, C64::new(f64::NAN, 0.0)),
        line,
    );
    assert!(!r.usable[2]);
    assert!(r.overshoot > 1.0, "{}", r.overshoot);
    let factor = per_line_of(&out, "ceiling_factor")[line];
    assert!((factor - 1.0 / r.overshoot).abs() < 1e-15);
    let h = s.frf.line(line);
    let x = full(&out.drive, line);
    let mut y2 = 0.0;
    for i in 0..4 {
        for j in 0..4 {
            y2 += (h[2 * 4 + i] * x[i * 4 + j] * h[2 * 4 + j].conj()).re;
        }
    }
    let bar = t2 * 10f64.powf(1.2) * 100.0;
    assert!((y2 / bar - 1.0).abs() < 1e-9, "{y2} against {bar}");
}

#[test]
fn a_line_with_every_control_bad_takes_no_step() {
    let s = &structures()[1].1;
    let line = 30;
    let (_, r) = one_loop_with(
        s,
        &[],
        |y, _| {
            for m in 0..6 {
                y.set(line, m, m, C64::new(f64::NAN, 0.0));
            }
        },
        line,
    );
    assert!(r.usable.iter().all(|&u| !u));
    assert!(r.u.iter().all(|&v| v == 0.0));
    assert_eq!(
        r.level, 0.0,
        "the level split's zero denominator takes no level"
    );
    assert_eq!(r.drive, r.base, "the drive is the base drive");
}

#[test]
fn a_zero_target_and_a_line_with_no_specification_are_defined() {
    let mut s = structures()[1].1.clone_run();
    let (line, quiet) = (30, 31);
    // Control 0 has a zero target on `line`; `quiet` has none at all.
    for m in 0..6 {
        for k in 0..6 {
            if m == 0 || k == 0 {
                s.setup.reference.set(line, m, k, C64::new(0.0, 0.0));
            }
            s.setup.reference.set(quiet, m, k, C64::new(0.0, 0.0));
        }
    }
    let (out, r) = one_loop_with(&s, &[], |_, _| {}, line);
    assert!(!r.usable[0]);
    assert!(r.usable[1..].iter().all(|&u| u));
    assert!(out
        .drive
        .line(quiet)
        .iter()
        .all(|v| *v == C64::new(0.0, 0.0)));
}

#[test]
fn a_zero_row_and_a_zero_column_of_the_frequency_response_are_defined() {
    let mut s = structures()[1].1.clone_run();
    let line = 30;
    for d in 0..4 {
        s.frf.set(line, 2, d, C64::new(0.0, 0.0));
    }
    for m in 0..6 {
        s.frf.set(line, m, 3, C64::new(0.0, 0.0));
    }
    let (_, r) = one_loop_with(&s, &[], |_, _| {}, line);
    assert!(!r.usable[2], "a control no drive reaches is left out");
    assert_eq!(
        (r.u[3], r.u[4 + 3]),
        (0.0, 0.0),
        "a drive that reaches nothing is not stepped"
    );
    assert!(r.u[..3].iter().any(|v| *v != 0.0));
}

#[test]
fn the_broyden_update_with_no_change_and_a_zero_denominator_is_defined() {
    // Learn from every step, however small: a line whose drive did not move
    // has du = 0, so du^T du = 0 and the update adds nothing.
    let s = &structures()[0].1;
    let line = 30;
    let mut law = law_with(
        &s.setup,
        &[
            ("min_du_db", ParamVal::Number(0.0)),
            ("convergence", ParamVal::Number(0.1)),
        ],
    );
    law.record_line(Some(line));
    let needs = law.needs().to_vec();
    let csd = cross(&s.plant, &s.sysid_drive);
    let inputs = s.inputs(&s.sysid_response, &s.sysid_drive, &csd, 0, &needs);
    let seed = law.seed(&inputs).expect("seed");
    // The measured response is exactly the target, so the first step is
    // zero, and the second loop learns from a step of zero.
    let target = s.setup.reference.clone();
    let mut drive = seed.drive.clone();
    for l in 1..=2u64 {
        let inputs = s.inputs(&target, &drive, &csd, l, &needs);
        drive = law.iterate(&inputs).expect("defined").drive;
    }
    let r = law.line_record().expect("recorded").clone();
    // The update ran and learned nothing (the prediction is zero, so is
    // every trust weight), so it is not counted.
    assert!(!r.updated);
    assert!(r.trust.iter().all(|&w| w == 0.0), "{:?}", r.trust);
    assert!(r.du[..2].iter().all(|v| v.abs() < 1e-15), "{:?}", r.du);
    assert!(r.s_updated.iter().all(|v| v.is_finite()));
    assert!(drive.is_finite());
}

#[test]
fn a_wild_measurement_cannot_carry_the_learned_sensitivity_past_its_bounds() {
    // Control 0 reads 30 dB high and low on alternate loops: the secant
    // update asks for entries far outside the model's; the first correction
    // bounds each entry by max(2, 2 |model entry|) (Choice 9) and the second
    // holds each row's magnitude sum within [0, 2].
    let s = &structures()[0].1;
    let line = 30;
    let mut law = law_with(&s.setup, &[("min_du_db", ParamVal::Number(0.0))]);
    law.record_line(Some(line));
    let needs = law.needs().to_vec();
    let csd = cross(&s.plant, &s.sysid_drive);
    let mut response = s.sysid_response.clone();
    let mut last = s.sysid_drive.clone();
    let mut bounded = 0;
    for l in 0..=8u64 {
        let inputs = s.inputs(&response, &last, &csd, l, &needs);
        let out = if l == 0 {
            law.seed(&inputs)
        } else {
            law.iterate(&inputs)
        }
        .expect("call");
        if l > 1 {
            let r = law.line_record().expect("recorded");
            assert!(r.updated, "loop {l}");
            let p = r.u.len();
            for (j, (v, m)) in r.s_updated.iter().zip(&r.s_model).enumerate() {
                let bound = 2f64.max(2.0 * m.abs());
                bounded += (v.abs() > bound) as usize;
                assert!(
                    r.s_corrected[j].abs() <= bound * (1.0 + 1e-12),
                    "loop {l} entry {j}"
                );
            }
            for m in 0..3 {
                let sum: f64 = r.s_corrected[m * p..m * p + 2].iter().sum();
                assert!(
                    (-1e-12..=2.0 + 1e-12).contains(&sum),
                    "loop {l} row {m}: {sum}"
                );
            }
        }
        last = out.drive;
        response = s.respond(&last, l + 1);
        let v = response.get(line, 0, 0);
        let k = if l % 2 == 0 { 1e3 } else { 1e-3 };
        response.set(line, 0, 0, v * k);
    }
    println!("entries the update carried past their bound: {bounded}");
    assert!(bounded > 0, "the bound never acted");
}

#[test]
fn the_skip_rule_leaves_a_line_whose_gains_barely_moved() {
    // With min_du_db 0.1, no line is updated after a step whose largest
    // drive gain change was under 0.1 dB (Choice 7), and some are skipped.
    let s = &structures()[1].1;
    let min_du_db = 0.1;
    let mut law = law_with(&s.setup, &[("min_du_db", ParamVal::Number(min_du_db))]);
    let run = closed_loop(&mut law, s, 10);
    let (mut updated, mut skipped) = (0, 0);
    for l in 2..run.outputs.len() {
        let flags = per_line_of(&run.outputs[l], "broyden_updated");
        let (now, before) = (&run.outputs[l - 1].drive, &run.outputs[l - 2].drive);
        for line in BAND.0..BAND.1 {
            let (a, b) = (now.diagonal(line), before.diagonal(line));
            let gain_db = (0..a.len())
                .map(|i| (10.0 * (a[i] / b[i]).log10()).abs())
                .fold(0.0f64, f64::max);
            if flags[line] == 1.0 {
                assert!(
                    gain_db >= min_du_db - 1e-9,
                    "loop {l} line {line}: {gain_db} dB"
                );
                updated += 1;
            } else if gain_db < min_du_db {
                skipped += 1;
            }
        }
    }
    println!("skip rule: {updated} line updates, {skipped} lines under {min_du_db} dB");
    assert!(updated > 0 && skipped > 0);
}

#[test]
fn a_call_that_cannot_be_answered_says_so() {
    let s = &structures()[0].1;
    let mut law = law_with(&s.setup, &[]);
    let needs = [MimoInput::LastDrive];
    let csd = cross(&s.plant, &s.sysid_drive);
    let inputs = s.inputs(&s.sysid_response, &s.sysid_drive, &csd, 1, &needs);
    assert!(law.iterate(&inputs).is_err(), "iterate before seed");
    law.seed(&inputs).expect("seed");
    let mut no_last = s.inputs(&s.sysid_response, &s.sysid_drive, &csd, 1, &needs);
    no_last.last_drive = None;
    assert!(law.iterate(&no_last).is_err());
    let mut bad = s.frf.clone();
    bad.set(30, 0, 0, C64::new(f64::NAN, 0.0));
    let mut inputs = s.inputs(&s.sysid_response, &s.sysid_drive, &csd, 1, &needs);
    inputs.frf = &bad;
    let e = law
        .iterate(&inputs)
        .expect_err("a frequency response that is not finite");
    assert!(e.0.contains("not finite"), "{e}");
    // A parameter outside its declaration is refused.
    assert!(
        BroydenCongruenceLaw::with_params(&s.setup, &[("ridge", ParamVal::Number(-1.0))]).is_err()
    );
}

#[test]
fn the_base_is_the_drive_as_played_when_it_was_not_played_as_commanded() {
    let s = &structures()[0].1;
    let line = 30;
    let mut law = law_with(&s.setup, &[]);
    law.record_line(Some(line));
    let needs = law.needs().to_vec();
    let csd = cross(&s.plant, &s.sysid_drive);
    let inputs = s.inputs(&s.sysid_response, &s.sysid_drive, &csd, 0, &needs);
    let seed = law.seed(&inputs).expect("seed");
    // Played 6 dB below the command on every line.
    let mut played = seed.drive.clone();
    played.scale_lines(&vec![0.25; LINES]).expect("scale");
    let response = s.respond(&played, 1);
    let inputs = s.inputs(&response, &played, &csd, 1, &needs);
    let out = law.iterate(&inputs).expect("loop");
    let r = law.line_record().expect("recorded");
    assert!(r.base_played);
    assert_eq!(r.base, full(&played, line));
    assert_eq!(per_line_of(&out, "base_played")[line], 1.0);
    // Within the tolerance it is the command.
    let mut law = law_with(&s.setup, &[("realize_tol_db", ParamVal::Number(7.0))]);
    law.record_line(Some(line));
    let inputs0 = s.inputs(&s.sysid_response, &s.sysid_drive, &csd, 0, &needs);
    law.seed(&inputs0).expect("seed");
    law.iterate(&inputs).expect("loop");
    let r = law.line_record().expect("recorded");
    assert!(!r.base_played);
    assert_eq!(r.base, full(&seed.drive, line));
}

trait CloneRun {
    fn clone_run(&self) -> Synthetic;
}

impl CloneRun for Synthetic {
    fn clone_run(&self) -> Synthetic {
        let mut s = run_on(
            self.seed,
            self.plant.clone(),
            self.frf.clone(),
            self.setup.reference.clone(),
            self.setup.band,
            self.eps,
        );
        s.coherence = self.coherence.clone();
        s
    }
}

// -------------------------------------------------------------------------
// 8. Lines the core holds back
// -------------------------------------------------------------------------

/// The core holds `held` lines of the played drive at `hold` (a fixed
/// drive, whatever the command) on the drives returned at loops `from` to
/// `to`; elsewhere the drive is played as commanded. Returns every command
/// and every drive played (`played[l]` is the `last_drive` of loop `l`).
fn run_with_hold(
    s: &Synthetic,
    loops: u64,
    held: std::ops::Range<usize>,
    hold: Option<(&HermitianStack, u64, u64)>,
) -> (
    Vec<hertzforge_sdm::law::MimoLoopOutputs>,
    Vec<HermitianStack>,
) {
    let mut law = law_with(&s.setup, &[]);
    let needs = law.needs().to_vec();
    let csd = cross(&s.plant, &s.sysid_drive);
    let mut response = s.sysid_response.clone();
    let mut last = s.sysid_drive.clone();
    let (mut outs, mut played) = (Vec::new(), Vec::new());
    for l in 0..=loops {
        let inputs = s.inputs(&response, &last, &csd, l, &needs);
        played.push(last.clone());
        let out = if l == 0 {
            law.seed(&inputs)
        } else {
            law.iterate(&inputs)
        }
        .unwrap_or_else(|e| panic!("loop {l}: {e}"));
        last = out.drive.clone();
        if let Some((fixed, from, to)) = hold {
            if (from..=to).contains(&l) {
                for line in held.clone() {
                    for r in 0..fixed.n() {
                        for c in r..fixed.n() {
                            last.set(line, r, c, fixed.get(line, r, c));
                        }
                    }
                }
            }
        }
        response = s.respond(&last, l + 1);
        outs.push(out);
    }
    (outs, played)
}

/// The largest `10 log10(y / t)` over the controls of `lines`, for the
/// noiseless response of `drive`.
fn worst_above(s: &Synthetic, drive: &HermitianStack, lines: std::ops::Range<usize>) -> f64 {
    let y = congruence(&s.plant, drive);
    let mut worst = f64::NEG_INFINITY;
    for line in lines {
        for m in 0..s.setup.controls {
            let (a, t) = (y.get(line, m, m).re, s.setup.reference.get(line, m, m).re);
            worst = worst.max(10.0 * (a / t).log10());
        }
    }
    worst
}

#[test]
fn a_line_the_core_holds_back_does_not_wind_up() {
    let s = &structures()[1].1;
    let held = 30..35;
    let (from, to, loops) = (6u64, 15u64, 20u64);
    let (free, _) = run_with_hold(s, loops, held.clone(), None);
    // The hold: 10 dB below what the test needs (the free run's drive at the
    // loop the hold starts), whatever the command.
    let mut fixed = free[from as usize].drive.clone();
    fixed.scale_lines(&vec![0.1; s.setup.lines]).expect("scale");
    let (outs, played) = run_with_hold(s, loops, held.clone(), Some((&fixed, from, to)));
    let db = |a: f64, b: f64| 10.0 * (a / b).log10();
    // During the hold: the command against the drive played on each held line.
    let mut widest: f64 = 0.0;
    for l in (from + 1)..=(to + 1) {
        for line in held.clone() {
            let (c, p) = (
                outs[l as usize].drive.trace(line),
                played[l as usize].trace(line),
            );
            widest = widest.max(db(c, p).abs());
        }
    }
    let before = worst_above(s, &outs[from as usize - 1].drive, held.clone());
    let after = worst_above(s, &outs[to as usize + 1].drive, held.clone());
    let counts: Vec<f64> = (0..=loops as usize)
        .map(|l| {
            match outs[l]
                .diagnostics
                .iter()
                .find(|d| d.key == "base_played_lines")
            {
                Some(d) => match d.data {
                    hertzforge_sdm::law::DiagnosticData::Scalar(v) => v,
                    _ => f64::NAN,
                },
                None => f64::NAN,
            }
        })
        .collect();
    println!(
        "hold of 10 dB on {} lines for {} loops: the command stood at most {widest:.2} dB from \
         the drive played; worst control above target before the hold {before:.2} dB, on the \
         first loop after release {after:.2} dB; lines counted per loop {counts:?}",
        held.len(),
        to - from + 1
    );
    assert!(after <= before + 1.0, "{after} dB against {before} dB");
    assert!(widest <= 3.0 + 3.0 + 1e-9, "{widest} dB");
    // The count: the lines whose played drive was more than 3 dB from the
    // command (the drive returned the loop before).
    let mut counted = 0.0;
    for l in 1..=loops as usize {
        let want = (s.setup.band.0..s.setup.band.1)
            .filter(|&line| db(played[l].trace(line), outs[l - 1].drive.trace(line)).abs() > 3.0)
            .count() as f64;
        assert_eq!(counts[l], want, "loop {l}");
        counted += want;
    }
    assert!(counted > 0.0);
    // Every line is independent of the others in this harness, and the
    // band's median is not moved by five lines of 48: the lines not held are
    // the free run's, to the bit.
    for l in 0..=loops as usize {
        for line in (s.setup.band.0..s.setup.band.1).filter(|l| !held.contains(l)) {
            assert_eq!(
                outs[l].drive.line(line),
                free[l].drive.line(line),
                "loop {l} line {line}"
            );
        }
    }
}

// -------------------------------------------------------------------------
// 9. The floor in drive units
// -------------------------------------------------------------------------

/// The 10 log10 errors of every control on every in-band line, for the
/// noiseless response of `drive`.
fn errors_db(s: &Synthetic, drive: &HermitianStack) -> Vec<f64> {
    let y = congruence(&s.plant, drive);
    let mut e = Vec::new();
    for line in s.setup.band.0..s.setup.band.1 {
        for m in 0..s.setup.controls {
            let (a, t) = (y.get(line, m, m).re, s.setup.reference.get(line, m, m).re);
            e.push(10.0 * (a / t).log10());
        }
    }
    e
}

#[test]
fn the_law_does_not_depend_on_the_units_of_the_frequency_response() {
    // The same structure with its frequency response, the true one and the
    // estimate, in units 1e-4, 1e3 and 1e4 times larger: the errors are the
    // same loop by loop, the drives scale by the inverse square, and no line
    // is ever at the floor (Choice 14).
    let base = &structures()[1].1;
    let loops = 12;
    let mut law = law_with(&base.setup, &[]);
    let reference_run = closed_loop(&mut law, base, loops);
    for k in [1e-4, 1.0, 1e3, 1e4] {
        let s = run_on(
            base.seed,
            scaled(&base.plant, c(k)),
            scaled(&base.frf, c(k)),
            base.setup.reference.clone(),
            BAND,
            base.eps,
        );
        let mut law = law_with(&s.setup, &[]);
        let run = closed_loop(&mut law, &s, loops);
        let (mut err, mut drive) = (0.0f64, 0.0f64);
        for l in 0..=loops {
            let (a, b) = (
                errors_db(&s, &run.outputs[l].drive),
                errors_db(base, &reference_run.outputs[l].drive),
            );
            for (x, y) in a.iter().zip(&b) {
                err = err.max((x - y).abs());
            }
            let (d, d0) = (&run.outputs[l].drive, &reference_run.outputs[l].drive);
            for line in BAND.0..BAND.1 {
                for (x, y) in d.line(line).iter().zip(d0.line(line)) {
                    drive = drive.max((x * (k * k) - y).norm() / d0.trace(line));
                }
            }
            let lifted = per_line_of(&run.outputs[l], "floor_lifted");
            assert!(lifted.iter().all(|&v| v == 0.0), "scale {k} loop {l}");
        }
        println!(
            "frequency response x {k:e}: errors differ by {err:.1e} dB, drives by {drive:.1e}"
        );
        assert!(err < 1e-9, "scale {k}: {err} dB");
        assert!(drive < 1e-9, "scale {k}: {drive}");
    }
}

/// The trace of the model drive `H^+ S_ref H^+^H` on `line`.
fn model_trace(s: &Synthetic, line: usize) -> f64 {
    let (plus, _) = linalg::pinv(&s.frf, Regularization::Truncated { rcond: 1e-3 }).expect("pinv");
    linalg::congruence(&plus, &s.setup.reference)
        .expect("congruence")
        .trace(line)
}

/// A line whose every control reads 10 dB high whatever the drive, on the
/// calls before `stuck_until`, and its true response after: the law steps
/// it down 20 dB a loop while the reading is stuck.
fn driven_down(
    stuck_until: u64,
) -> (
    Vec<LineRecord>,
    Vec<hertzforge_sdm::law::MimoLoopOutputs>,
    f64,
) {
    let s = &structures()[1].1;
    let line = 30;
    let mut law = law_with(
        &s.setup,
        &[
            ("max_step_db", ParamVal::Number(20.0)),
            ("convergence", ParamVal::Number(1.0)),
        ],
    );
    law.record_line(Some(line));
    let needs = law.needs().to_vec();
    let csd = cross(&s.plant, &s.sysid_drive);
    let mut response = s.sysid_response.clone();
    let mut last = s.sysid_drive.clone();
    let (mut recs, mut outs) = (Vec::new(), Vec::new());
    for l in 0..=12u64 {
        let inputs = s.inputs(&response, &last, &csd, l, &needs);
        let out = if l == 0 {
            law.seed(&inputs)
        } else {
            law.iterate(&inputs)
        }
        .expect("call");
        if l > 0 {
            recs.push(law.line_record().expect("recorded").clone());
        }
        last = out.drive.clone();
        outs.push(out);
        response = s.respond(&last, l + 1);
        if l + 1 < stuck_until {
            for m in 0..6 {
                let t = s.setup.reference.get(line, m, m).re;
                response.set(line, m, m, C64::new(10.0 * t, 0.0));
            }
        }
    }
    (recs, outs, model_trace(s, line))
}

#[test]
fn the_floor_lifts_a_line_driven_down_even_against_the_ceiling() {
    let line = 30;
    // Stuck for eight calls: the drive goes 160 dB down; then the true
    // response, tiny, the law steps up 20 dB, and the floor lifts the line
    // the rest of the way.
    let (recs, outs, model) = driven_down(9);
    let floor = 1e-8 * model;
    let lifted: Vec<usize> = (1..outs.len())
        .filter(|&l| per_line_of(&outs[l], "floor_lifted")[line] == 1.0)
        .collect();
    assert!(!lifted.is_empty(), "the floor never acted");
    for &l in &lifted {
        let tr = trace_of(&recs[l - 1].drive);
        assert!(
            (tr / floor - 1.0).abs() < 1e-9,
            "loop {l}: {tr} against {floor}"
        );
    }
    // Stuck on every call: the ceiling scales the line down each loop, below
    // the floor, and the floor, which has the last word, lifts it back to
    // the floor: never below it.
    let (recs, outs, _) = driven_down(u64::MAX);
    let mut overruled = Vec::new();
    for l in 1..outs.len() {
        let r = &recs[l - 1];
        let tr = trace_of(&r.drive);
        assert!(tr >= floor * (1.0 - 1e-9), "loop {l}: below the floor");
        let ceiling = per_line_of(&outs[l], "ceiling_factor")[line];
        if ceiling * trace_of(&r.applied) < floor * (1.0 - 1e-9) {
            assert_eq!(per_line_of(&outs[l], "floor_lifted")[line], 1.0, "loop {l}");
            assert!((tr / floor - 1.0).abs() < 1e-9, "loop {l}");
            overruled.push(l);
        }
    }
    assert!(!overruled.is_empty());
    println!("lifted to the floor on loops {lifted:?}; over the ceiling on {overruled:?}");
}

/// A closed loop in which control 0 on `stuck` reads 40 dB above its target
/// whatever the drive, on the calls before `stuck_until`, and truly after.
fn run_with_stuck(
    s: &Synthetic,
    loops: u64,
    stuck: std::ops::Range<usize>,
    stuck_until: u64,
) -> Vec<hertzforge_sdm::law::MimoLoopOutputs> {
    let mut law = law_with(&s.setup, &[]);
    let needs = law.needs().to_vec();
    let csd = cross(&s.plant, &s.sysid_drive);
    let mut response = s.sysid_response.clone();
    let mut last = s.sysid_drive.clone();
    let mut outs = Vec::new();
    for l in 0..=loops {
        let inputs = s.inputs(&response, &last, &csd, l, &needs);
        let out = if l == 0 {
            law.seed(&inputs)
        } else {
            law.iterate(&inputs)
        }
        .unwrap_or_else(|e| panic!("loop {l}: {e}"));
        last = out.drive.clone();
        response = s.respond(&last, l + 1);
        if l + 1 < stuck_until {
            for line in stuck.clone() {
                let t = s.setup.reference.get(line, 0, 0).re;
                response.set(line, 0, 0, C64::new(1e4 * t, 0.0));
            }
        }
        outs.push(out);
    }
    outs
}

#[test]
fn a_reading_stuck_high_leaves_the_line_at_the_floor_and_it_recovers() {
    let s = &structures()[1].1;
    let stuck = 30..33;
    let (release, loops) = (401u64, 480u64);
    let free = run_with_stuck(s, loops, stuck.clone(), 0);
    let outs = run_with_stuck(s, loops, stuck.clone(), release);
    let db = |a: f64, b: f64| 10.0 * (a / b).log10();
    // Every value returned is finite.
    for (l, out) in outs.iter().enumerate() {
        for line in 0..s.setup.lines {
            assert!(
                out.drive
                    .line(line)
                    .iter()
                    .all(|v| v.re.is_finite() && v.im.is_finite()),
                "loop {l} line {line}"
            );
        }
        for d in &out.diagnostics {
            match &d.data {
                hertzforge_sdm::law::DiagnosticData::Scalar(v) => {
                    assert!(v.is_finite(), "loop {l} {}", d.key)
                }
                hertzforge_sdm::law::DiagnosticData::PerLine(v) => {
                    assert!(v.iter().all(|x| x.is_finite()), "loop {l} {}", d.key)
                }
                _ => {}
            }
        }
    }
    // While stuck: the drive falls to the floor, settles on it and never
    // goes below it.
    let mut settled_from = 0;
    for line in stuck.clone() {
        let floor = 1e-8 * model_trace(s, line);
        let mut first = None;
        for l in 1..release as usize {
            let tr = outs[l].drive.trace(line);
            assert!(
                tr >= floor * (1.0 - 1e-9),
                "loop {l} line {line}: below the floor"
            );
            if (tr / floor - 1.0).abs() < 1e-9 {
                first.get_or_insert(l);
                assert_eq!(per_line_of(&outs[l], "floor_lifted")[line], 1.0);
            } else {
                assert!(
                    first.is_none(),
                    "loop {l} line {line}: left the floor while stuck"
                );
            }
        }
        let first = first.expect("never reached the floor");
        settled_from = settled_from.max(first);
    }
    // While stuck the learning goes on taking in the stuck reading.
    let updates: f64 = (1..release as usize)
        .map(|l| {
            stuck
                .clone()
                .map(|line| per_line_of(&outs[l], "broyden_updated")[line])
                .sum::<f64>()
        })
        .sum();
    println!("updates on the stuck lines while stuck: {updates}");
    // After release: back within `close` dB of the free run's drive on every
    // stuck line, and staying there, no faster than the 3 dB step limit
    // allows from the floor. The drive does not return to the free run's
    // exactly: on line 32 it overshoots it. Measured to loop 480 at this
    // seed, line 32 first comes within 1 dB 40 loops after release on macOS
    // and 38 on Windows, then reaches +0.92 dB on macOS and +1.21 dB on
    // Windows (at loop 480), so a 1 dB bound held on macOS only. Over twelve
    // noise seeds run to loop 800 on each platform, the largest distance
    // from the free run after line 32 first came within 1 dB was 1.40 dB on
    // macOS and 1.48 dB on Windows, and within 2 dB every stuck line
    // recovered in 31 to 38 loops. The two platforms' runs differ from the
    // start: the structure's normal draws use the system `ln` and `cos`, and
    // the first drive's pseudo-inverse differs in the last bit even on
    // identical inputs.
    let close = 2.0;
    let mut recovered = 0;
    let mut fastest = 0;
    for line in stuck.clone() {
        let floor = 1e-8 * model_trace(s, line);
        let want = free[loops as usize].drive.trace(line);
        let gap = db(want, floor);
        let allowed = ((gap - close) / 3.0).ceil() as usize;
        let took = (release as usize..=loops as usize)
            .find(|&l| {
                (release as usize..=loops as usize)
                    .skip(l - release as usize)
                    .all(|k| {
                        db(outs[k].drive.trace(line), free[k].drive.trace(line)).abs() <= close
                    })
            })
            .expect("never recovered")
            - (release as usize - 1);
        // Each loop raises the drive by at most 3 dB.
        for l in release as usize..=loops as usize {
            let rise = db(outs[l].drive.trace(line), outs[l - 1].drive.trace(line));
            assert!(rise <= 3.0 + 1e-9, "loop {l} line {line}: {rise} dB");
        }
        assert!(
            took >= allowed,
            "line {line}: {took} loops against {allowed}"
        );
        // And it climbs at no less than half the step limit on average: the
        // limit binds on the drive that moves most, so the trace moves less.
        assert!(
            took <= 2 * allowed,
            "line {line}: {took} loops against {allowed}"
        );
        recovered = recovered.max(took);
        fastest = fastest.max(allowed);
        println!("line {line}: floor {gap:.2} dB below the drive needed; {took} loops to recover, the step limit allows no fewer than {allowed}");
    }
    // The other lines: the free run's, to the bit, outside the stuck lines'
    // resonance window (lines within it may see their rows).
    let mut differ = Vec::new();
    for l in 0..=loops as usize {
        for line in (s.setup.band.0..s.setup.band.1).filter(|l| !stuck.contains(l)) {
            if outs[l].drive.line(line) != free[l].drive.line(line) {
                differ.push(line);
            }
        }
    }
    differ.sort_unstable();
    differ.dedup();
    println!("on the floor from loop {settled_from}; recovered within {recovered} loops (step limit {fastest}); other lines that differ {differ:?}");
    assert!(differ.is_empty(), "{differ:?}");
}

/// The 6 x 4 structure of the second review: singular values 1, 0.3, 0.1
/// and 1e-6 on every line, the estimate within 1e-6 of it, and a random
/// reference it cannot reach.
fn near_singular() -> Synthetic {
    let plant = plant_with_singular_values(41, 6, 4, LINES, &[1.0, 0.3, 0.1, 1e-6]);
    let mut s = Synthetic::with_plant(41, plant.clone(), BAND, 0.01);
    s.frf = perturbed(&plant, 41, 1e-6);
    s
}

#[test]
fn the_floor_comes_from_the_truncated_inverse_whatever_the_regularization() {
    let s = near_singular();
    let svd = linalg::svd(&s.frf).expect("svd");
    let k = svd.k();
    for (name, given) in [
        ("truncated", vec![]),
        (
            "none",
            vec![("regularization", ParamVal::Choice("none".into()))],
        ),
        (
            "tikhonov 1e-6",
            vec![
                ("regularization", ParamVal::Choice("tikhonov".into())),
                ("regularization_amount", ParamVal::Number(1e-6)),
            ],
        ),
    ] {
        let mut law = law_with(&s.setup, &given);
        let run = closed_loop(&mut law, &s, 30);
        let mut lifts = 0;
        for line in BAND.0..BAND.1 {
            let t = s.setup.reference.trace(line);
            let floor = 1e-8 * model_trace(&s, line);
            // The bound (Choice 14): a drive at the floor answers, through
            // the seed's frequency response, with at most 1e-8 x 1e6 of the
            // target's total.
            let smax = svd.s[line * k];
            assert!(
                smax * smax * floor <= 1e-2 * t * (1.0 + 1e-9),
                "{name} line {line}"
            );
            // The seed lifted nothing: every first drive is at or above it.
            assert!(
                run.outputs[0].drive.trace(line) >= floor,
                "{name} line {line}"
            );
            for l in 1..=30 {
                if run.per_line(l, "floor_lifted")[line] == 1.0 {
                    lifts += 1;
                    let x = full(&run.outputs[l].drive, line);
                    let h = s.frf.line(line);
                    let mut y = 0.0;
                    for m in 0..6 {
                        for i in 0..4 {
                            for j in 0..4 {
                                y += (h[m * 4 + i] * x[i * 4 + j] * h[m * 4 + j].conj()).re;
                            }
                        }
                    }
                    assert!(y <= 1e-2 * t * (1.0 + 1e-9), "{name} loop {l} line {line}");
                }
            }
        }
        // The floor never lifted a line, so each run is the one it would be
        // with the floor's lift taken out: the floor added nothing to the
        // highest control.
        let top = worst_above(&s, run.last(), BAND.0..BAND.1);
        println!("{name}: {lifts} lifts in 30 loops; highest control {top:.2} dB over its target");
        assert_eq!(lifts, 0, "{name}");
    }
}

#[test]
fn an_estimate_that_falls_on_one_call_does_not_lift_the_floor() {
    let s = &structures()[1].1;
    let lines = 30..35;
    let (at, loops) = (6u64, 8u64);
    // The resonance cut reads each control's row against its neighbours',
    // so a scaled estimate moves the cut on lines near the scaled ones; with
    // the cut off every other line is independent of them.
    let run = |a: Option<f64>, cut: bool| {
        let off = [("resonance_sensitivity", ParamVal::Number(0.0))];
        let mut law = law_with(&s.setup, if cut { &[] } else { &off });
        let needs = law.needs().to_vec();
        let mut frf = s.frf.clone();
        if let Some(a) = a {
            for line in lines.clone() {
                for m in 0..6 {
                    for d in 0..4 {
                        frf.set(line, m, d, s.frf.get(line, m, d) * a);
                    }
                }
            }
        }
        let csd = cross(&s.plant, &s.sysid_drive);
        let mut response = s.sysid_response.clone();
        let mut last = s.sysid_drive.clone();
        let mut outs = Vec::new();
        for l in 0..=loops {
            let mut inputs = s.inputs(&response, &last, &csd, l, &needs);
            if l == at {
                inputs.frf = &frf;
            }
            let out = if l == 0 {
                law.seed(&inputs)
            } else {
                law.iterate(&inputs)
            }
            .unwrap_or_else(|e| panic!("loop {l}: {e}"));
            last = out.drive.clone();
            response = s.respond(&last, l + 1);
            outs.push(out);
        }
        outs
    };
    let (free, free_cut) = (run(None, false), run(None, true));
    // The estimate scaled by `a` on five lines, on loop 6 only, as in the
    // review: the model drive there `1 / a^2` times larger.
    for a in [1e-5, 1e-6] {
        let (outs, cut) = (run(Some(a), false), run(Some(a), true));
        let l = at as usize;
        for o in [&outs[l], &cut[l]] {
            assert!(
                per_line_of(o, "floor_lifted").iter().all(|&v| v == 0.0),
                "{a}: lifted"
            );
        }
        let mut moved: f64 = 0.0;
        for line in (BAND.0..BAND.1).filter(|x| !lines.contains(x)) {
            moved = moved.max(
                (10.0 * (cut[l].drive.trace(line) / free_cut[l].drive.trace(line)).log10()).abs(),
            );
            assert_eq!(
                outs[l].drive.line(line),
                free[l].drive.line(line),
                "{a} line {line}"
            );
        }
        // On the scaled lines: the law's own step from the scaled estimate,
        // which is the free run's, the step being independent of the
        // estimate's units (the units test above), now that the floor is too.
        for line in lines.clone() {
            let d = 10.0 * (outs[l].drive.trace(line) / free[l].drive.trace(line)).log10();
            assert!(d.abs() < 1e-9, "{a} line {line}: {d} dB");
        }
        let db: Vec<String> = lines
            .clone()
            .map(|line| {
                format!(
                    "{:.1e}",
                    10.0 * (outs[l].drive.trace(line) / free[l].drive.trace(line)).log10()
                )
            })
            .collect();
        println!(
            "estimate x {a:e} on loop {at}: no line lifted; drive on the scaled lines \
             against the free run {db:?} dB; with the resonance cut on, other lines moved at \
             most {moved:.3} dB"
        );
    }
}

#[test]
fn the_floor_is_set_by_each_seed_and_a_failed_seed_leaves_it() {
    let s = &structures()[1].1;
    let line = 30;
    let needs = [MimoInput::LastDrive];
    let csd = cross(&s.plant, &s.sysid_drive);
    let floor = 1e-8 * model_trace(s, line);
    let mut law = law_with(&s.setup, &[]);
    let mut twin = law_with(&s.setup, &[]);
    law.record_line(Some(line));
    twin.record_line(Some(line));
    let inputs = s.inputs(&s.sysid_response, &s.sysid_drive, &csd, 0, &needs);
    let first = law.seed(&inputs).expect("seed");
    twin.seed(&inputs).expect("seed");
    let got = law.line_record().expect("recorded").floor;
    assert!((got / floor - 1.0).abs() < 1e-12, "{got} against {floor}");
    // Kept by iterate, whatever this call's estimate.
    let louder = scaled(&s.frf, c(10.0));
    let response = s.respond(&first.drive, 1);
    let mut inputs = s.inputs(&response, &first.drive, &csd, 1, &needs);
    inputs.frf = &louder;
    law.iterate(&inputs).expect("iterate");
    twin.iterate(&inputs).expect("iterate");
    assert_eq!(law.line_record().expect("recorded").floor, got);
    // A failed seed: the estimate 10 times larger, so every floor it formed
    // would be 100 times lower, and the band's last line so small that its
    // first drive is not finite, which fails after the floors are formed.
    let mut bad = louder.clone();
    for m in 0..6 {
        for d in 0..4 {
            bad.set(BAND.1 - 1, m, d, s.frf.get(BAND.1 - 1, m, d) * 1e-200);
        }
    }
    let mut inputs = s.inputs(&response, &first.drive, &csd, 0, &needs);
    inputs.frf = &bad;
    let e = law.seed(&inputs).expect_err("not finite");
    assert!(e.0.contains("not finite"), "{e}");
    // The law goes on as its twin, which never saw the failed seed.
    let inputs = s.inputs(&response, &first.drive, &csd, 2, &needs);
    let (oa, ot) = (
        law.iterate(&inputs).expect("law"),
        twin.iterate(&inputs).expect("twin"),
    );
    assert_eq!(oa.drive, ot.drive);
    assert_eq!(law.line_record(), twin.line_record());
    assert_eq!(law.line_record().expect("recorded").floor, got);
    // A new seed with the larger estimate sets the floor again.
    let mut inputs = s.inputs(&response, &first.drive, &csd, 0, &needs);
    inputs.frf = &louder;
    law.seed(&inputs).expect("seed");
    let again = law.line_record().expect("recorded").floor;
    assert!(
        (again / (got / 100.0) - 1.0).abs() < 1e-9,
        "{again} against {}",
        got / 100.0
    );
}

fn trace_of(x: &[C64]) -> f64 {
    let n = (x.len() as f64).sqrt() as usize;
    (0..n).map(|i| x[i * n + i].re).sum()
}

// -------------------------------------------------------------------------
// 10. A failed call changes nothing
// -------------------------------------------------------------------------

#[test]
fn a_call_that_fails_leaves_the_law_as_it_was() {
    let s = &structures()[1].1;
    let needs = [MimoInput::LastDrive];
    let csd = cross(&s.plant, &s.sysid_drive);
    let mut a = law_with(&s.setup, &[]);
    let mut b = law_with(&s.setup, &[]);
    let mut response = s.sysid_response.clone();
    let mut last = s.sysid_drive.clone();
    for l in 0..=3u64 {
        let inputs = s.inputs(&response, &last, &csd, l, &needs);
        let (oa, ob) = if l == 0 {
            (a.seed(&inputs), b.seed(&inputs))
        } else {
            (a.iterate(&inputs), b.iterate(&inputs))
        };
        let (oa, ob) = (oa.expect("a"), ob.expect("b"));
        assert_eq!(oa.drive, ob.drive);
        last = oa.drive;
        response = s.respond(&last, l + 1);
    }
    // The played drive 20 dB above the command, so every line builds on it,
    // with one entry not finite on the band's last line: the call fails
    // after every other line has been stepped.
    let mut wild = last.clone();
    wild.scale_lines(&vec![100.0; LINES]).expect("scale");
    wild.set(BAND.1 - 1, 0, 1, C64::new(f64::INFINITY, 0.0));
    let bad = s.inputs(&response, &wild, &csd, 4, &needs);
    let e = a.iterate(&bad).expect_err("not finite");
    assert!(e.0.contains("not finite"), "{e}");
    let good = s.inputs(&response, &last, &csd, 4, &needs);
    let (oa, ob) = (a.iterate(&good).expect("a"), b.iterate(&good).expect("b"));
    assert_eq!(oa.drive, ob.drive, "the drives");
    assert_eq!(oa.diagnostics.len(), ob.diagnostics.len());
    for (x, y) in oa.diagnostics.iter().zip(&ob.diagnostics) {
        assert_eq!(x.key, y.key);
        assert_eq!(
            format!("{:?}", x.data),
            format!("{:?}", y.data),
            "{}",
            x.key
        );
    }
}

// -------------------------------------------------------------------------
// 11. Learning on a resonant structure
// -------------------------------------------------------------------------

/// A resonant structure: four modes at lines 15, 26, 38 and 49 (2 percent
/// damping) with random real shapes, and a small random residual.
fn resonant(seed: u64, nc: usize, nd: usize, zeta: f64) -> MatrixStack {
    let mut rng = Rng::new(seed);
    let modes = [(15.0, zeta), (26.0, zeta), (38.0, zeta), (49.0, zeta)];
    let shapes: Vec<(Vec<f64>, Vec<f64>)> = modes
        .iter()
        .map(|_| {
            (
                (0..nc).map(|_| rng.normal()).collect(),
                (0..nd).map(|_| rng.normal()).collect(),
            )
        })
        .collect();
    let residual: Vec<C64> = (0..nc * nd).map(|_| rng.cnormal() * 0.02).collect();
    let mut h = MatrixStack::zeros(LINES, nc, nd);
    for line in 0..LINES {
        let w = line as f64;
        for m in 0..nc {
            for d in 0..nd {
                let mut v = residual[m * nd + d];
                for ((wr, z), (phi, psi)) in modes.iter().zip(&shapes) {
                    let den = C64::new(wr * wr - w * w, 2.0 * z * wr * w);
                    v += C64::new(phi[m] * psi[d] * wr * wr * 0.02, 0.0) / den;
                }
                h.set(line, m, d, v);
            }
        }
    }
    h
}

/// The rms and the worst of the errors, dB.
fn rms_worst(e: &[f64]) -> (f64, f64) {
    let rms = (e.iter().map(|v| v * v).sum::<f64>() / e.len() as f64).sqrt();
    (rms, e.iter().fold(0.0f64, |w, v| w.max(v.abs())))
}

#[test]
fn learning_on_a_resonant_structure() {
    // A 5 percent complex error in the estimate, 30 loops, defaults, learning
    // on (broyden_gain 0.6) against off (0). Measured: with a reference the
    // structure can reach exactly, the first drive is already right and
    // learning makes no difference; with a random reference, whose
    // autospectra the first drive's coherence cannot all meet, the learned
    // sensitivity takes the loop much further.
    for (nc, nd, seed, zeta, reach) in [
        (6, 4, 61u64, 0.02, true),
        (12, 6, 62, 0.02, true),
        (6, 4, 61, 0.02, false),
        (12, 6, 62, 0.005, false),
    ] {
        let plant = resonant(seed, nc, nd, zeta);
        let reference = if reach {
            reachable_reference(&plant, seed, BAND)
        } else {
            Synthetic::with_plant(seed, plant.clone(), BAND, 0.01)
                .setup
                .reference
        };
        let frf = perturbed(&plant, seed, 0.05);
        let s = run_on(seed, plant, frf, reference, BAND, 0.01);
        let mut result = Vec::new();
        for gain in [0.6, 0.0] {
            let mut law = law_with(&s.setup, &[("broyden_gain", ParamVal::Number(gain))]);
            let run = closed_loop(&mut law, &s, 30);
            let (rms, worst) = rms_worst(&errors_db(&s, run.last()));
            let updates = scalar_of(run.outputs.last().expect("ran"), "broyden_updates");
            println!(
                "{nc} x {nd}, damping {zeta}, reachable {reach}, broyden_gain {gain}: \
                 after 30 loops rms {rms:.3} dB, worst {worst:.3} dB, {updates} updates"
            );
            result.push((rms, updates));
        }
        let ((on, n_on), (off, n_off)) = (result[0], result[1]);
        assert!(n_on > 0.0 && n_off == 0.0);
        if reach {
            assert!((on - off).abs() < 0.005, "{on} against {off}");
        } else {
            assert!(off - on > 0.5, "{on} against {off}");
        }
    }
}
