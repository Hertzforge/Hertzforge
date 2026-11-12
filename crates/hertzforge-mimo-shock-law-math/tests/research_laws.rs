//! The law requirements RL1 to RL8, fixed in the experiment log before the
//! code that could meet them existed, and the seam's own checks. Plants are the
//! research plants; the ladder is -12, -9, -6, -3 dB then 0 dB; the model is
//! identified at -20 dB; noise is absolute, a fraction of the reference's rms.

use hertzforge_mimo_shock_law_math::inverse::{InverseExtras, InverseLaw, InverseParams};
use hertzforge_mimo_shock_law_math::modal::{fit_modal, FitOptions, ModalModel};
use hertzforge_mimo_shock_law_math::provider::{
    FrozenModel, ModelProvider, MultiSecantModel, Observation, PoleTrackModel, SecantOptions,
    TrackOptions,
};
use hertzforge_mimo_shock_law_math::reference_plant::{Rng, FS_HZ, LADDER_DB, RECORD_LEN};
use hertzforge_mimo_shock_law_math::research_plant::{
    identify_h1, research_setup_signed, run_research_ladder, LevelPlant, ResearchRow,
};
use hertzforge_mimo_shock_law_math::shape::{ShapeExtras, ShapeLaw, ShapeParams, ShapeSafety};
use hertzforge_mimo_shock_law_math::spectrum::apply;
use hertzforge_mimo_shock_law_math::{LawSteps, MatrixStack, Setup, C64};

const S_ID: f64 = 0.1;
const SEED: u64 = 20_261_111;

fn band() -> Vec<bool> {
    let df = FS_HZ / RECORD_LEN as f64;
    (0..RECORD_LEN / 2 + 1)
        .map(|k| k > 0 && (80.0..=2400.0).contains(&(k as f64 * df)))
        .collect()
}

fn polarity(mixed: bool) -> Vec<f64> {
    (0..4)
        .map(|c| if mixed && c % 2 == 1 { -1.0 } else { 1.0 })
        .collect()
}

fn plants() -> Vec<(&'static str, LevelPlant)> {
    vec![
        ("8% fixed", LevelPlant::four_by_four_damped(0.08, 0.0, 0.02)),
        ("5% x2", LevelPlant::four_by_four_damped(0.05, 1.0, 0.02)),
        ("5% fixed", LevelPlant::four_by_four_damped(0.05, 0.0, 0.02)),
    ]
}

/// The setup and the modal model: the truth at -20 dB, or the realistic
/// identification and its fit.
fn case(plant: &LevelPlant, mixed: bool, fitted: bool) -> (Setup, ModalModel) {
    if fitted {
        let h = identify_h1(plant, S_ID, 16, 0.05, 7).unwrap();
        let fit = fit_modal(&h, FS_HZ, &FitOptions::default()).unwrap();
        (research_setup_signed(h, &polarity(mixed)), fit.model)
    } else {
        (
            research_setup_signed(plant.frf(S_ID, FS_HZ, RECORD_LEN), &polarity(mixed)),
            plant.modal_truth(S_ID),
        )
    }
}

fn tracker(setup: &Setup, modal: &ModalModel, opts: TrackOptions) -> Box<dyn ModelProvider> {
    let h = setup.plant.frf.clone();
    Box::new(
        PoleTrackModel::new(
            modal.clone(),
            h.clone(),
            band(),
            FS_HZ,
            opts,
            Box::new(MultiSecantModel::new(h, band(), SecantOptions::default())),
        )
        .unwrap(),
    )
}

fn shape_tracked(setup: &Setup, modal: &ModalModel, opts: TrackOptions) -> ShapeLaw {
    ShapeLaw::with_options(
        setup.clone(),
        ShapeParams::default(),
        ShapeExtras {
            provider: Some(tracker(setup, modal, opts)),
            ..Default::default()
        },
    )
    .unwrap()
}

fn run(
    law: &mut dyn LawSteps,
    setup: &Setup,
    plant: &LevelPlant,
    ladder: &[f64],
    noise: f64,
) -> Vec<ResearchRow> {
    let mut resp = |d: &[Vec<f64>], s: f64| plant.respond(d, s, FS_HZ);
    run_research_ladder(law, setup, &mut resp, ladder, noise, SEED, None).unwrap()
}

/// `(p5 over, p8 worst)` of `shape` with pole tracking and extrapolation.
fn rl1_numbers(plant: &LevelPlant, mixed: bool, fitted: bool, noise: f64) -> (f64, f64) {
    let (setup, modal) = case(plant, mixed, fitted);
    let mut law = shape_tracked(&setup, &modal, TrackOptions::default());
    let rows = run(&mut law, &setup, plant, &LADDER_DB, noise);
    (rows[4].over_db, rows[7].worst_db)
}

fn check_rl(fitted: bool) -> Vec<String> {
    let mut fails = Vec::new();
    for (name, plant) in plants() {
        for mixed in [false, true] {
            let (o0, w0) = rl1_numbers(&plant, mixed, fitted, 0.0);
            let (o3, w3) = rl1_numbers(&plant, mixed, fitted, 0.30);
            eprintln!(
                "{name:9} {:8} {}: clean p5 over {o0:+.2} p8 {w0:.2}; noise 30% p5 over {o3:+.2} p8 {w3:.2}",
                if mixed { "mixed" } else { "in phase" },
                if fitted { "fitted" } else { "true" }
            );
            if o0 >= 3.0 || w0 > 1.5 {
                fails.push(format!("{name} {mixed} clean"));
            }
            if (o3 - o0).abs() > 1.0 || (w3 - w0).abs() > 1.0 {
                fails.push(format!("{name} {mixed} noise"));
            }
        }
    }
    fails
}

#[test]
fn mimo_shock_rl1_rl2_pole_tracking_with_the_true_model() {
    let fails = check_rl(false);
    assert!(fails.is_empty(), "RL1/RL2 fail: {fails:?}");
}

#[test]
fn mimo_shock_rl3_pole_tracking_with_a_fitted_model() {
    let fails = check_rl(true);
    assert!(fails.is_empty(), "RL3 fails: {fails:?}");
}

/// Without tracking, `shape` over-tests the first full-level pulse by more
/// than 3 dB on the same plants: the requirement is not met by the plants.
#[test]
fn mimo_shock_rl1_is_not_met_without_tracking() {
    let plant = LevelPlant::four_by_four_damped(0.08, 0.0, 0.02);
    let (setup, _) = case(&plant, true, false);
    let mut law = ShapeLaw::new(setup.clone(), ShapeParams::default()).unwrap();
    let rows = run(&mut law, &setup, &plant, &LADDER_DB, 0.0);
    assert!(rows[4].over_db > 3.0, "{}", rows[4].over_db);
}

/// RL5: 100 pulses on the 4 x 4 at 8% without tracking: with the clamp the
/// gains stay within 12 dB at every pulse and the stall is flagged by p8;
/// without it they wind past the clamp.
#[test]
fn mimo_shock_rl5_shape_gain_clamp_and_stall_flag() {
    let plant = LevelPlant::four_by_four_damped(0.08, 0.0, 0.02);
    let ladder: Vec<f64> = [-12.0, -9.0, -6.0, -3.0]
        .into_iter()
        .chain([0.0; 96])
        .collect();
    for mixed in [false, true] {
        let (setup, _) = case(&plant, mixed, false);
        let mut safe = ShapeLaw::with_options(
            setup.clone(),
            ShapeParams::default(),
            ShapeExtras {
                safety: ShapeSafety::recommended(),
                ..Default::default()
            },
        )
        .unwrap();
        let rows = run(&mut safe, &setup, &plant, &ladder, 0.0);
        let cmax = rows
            .iter()
            .map(|r| r.value("gain_extent_db"))
            .fold(0.0f64, f64::max);
        let first = rows
            .iter()
            .position(|r| r.value("stalled_points") > 0.0)
            .map(|k| k + 1);
        eprintln!("RL5 {mixed}: clamped C max {cmax:.2} dB, first stall flag p{first:?}, worst p100 {:.2}", rows[99].worst_db);
        assert!(cmax <= 12.0 + 1e-9, "clamp exceeded: {cmax}");
        assert!(
            first.is_some_and(|k| k <= 8),
            "stall not flagged by p8: {first:?}"
        );
        let mut free = ShapeLaw::with_options(
            setup.clone(),
            ShapeParams::default(),
            ShapeExtras::default(),
        )
        .unwrap();
        let rows = run(&mut free, &setup, &plant, &ladder, 0.0);
        let cfree = rows
            .iter()
            .map(|r| r.value("gain_extent_db"))
            .fold(0.0f64, f64::max);
        eprintln!("RL5 {mixed}: unclamped C max {cfree:.1} dB");
        assert!(
            cfree > 12.0,
            "the clamp's test needs a plant that winds up: {cfree}"
        );
    }
}

/// `with_options` with no provider and the safety off drives exactly as
/// `new` does (the compatibility rule).
#[test]
fn mimo_shock_with_options_default_drives_as_new() {
    let plant = LevelPlant::four_by_four_damped(0.08, 0.0, 0.02);
    let (setup, _) = case(&plant, true, false);
    let mut a = ShapeLaw::new(setup.clone(), ShapeParams::default()).unwrap();
    let mut b = ShapeLaw::with_options(
        setup.clone(),
        ShapeParams::default(),
        ShapeExtras::default(),
    )
    .unwrap();
    let ra = run(&mut a, &setup, &plant, &LADDER_DB[..8], 0.0);
    let rb = run(&mut b, &setup, &plant, &LADDER_DB[..8], 0.0);
    for (x, y) in ra.iter().zip(&rb) {
        assert_eq!(x.worst_db.to_bits(), y.worst_db.to_bits());
        assert_eq!(x.drive_rms.to_bits(), y.drive_rms.to_bits());
    }
    let mut c = InverseLaw::new(setup.clone(), InverseParams::default()).unwrap();
    let mut d = InverseLaw::with_options(
        setup.clone(),
        InverseParams::default(),
        InverseExtras::default(),
    )
    .unwrap();
    let rc = run(&mut c, &setup, &plant, &LADDER_DB[..8], 0.0);
    let rd = run(&mut d, &setup, &plant, &LADDER_DB[..8], 0.0);
    for (x, y) in rc.iter().zip(&rd) {
        assert_eq!(x.worst_db.to_bits(), y.worst_db.to_bits());
    }
}

fn random_spectra(rows: usize, lines: usize, rng: &mut Rng) -> Vec<Vec<C64>> {
    (0..rows)
        .map(|_| {
            (0..lines)
                .map(|_| C64::new(rng.normal(), rng.normal()))
                .collect()
        })
        .collect()
}

/// The multi-secant reproduces every stored pair of its level on every line
/// (up to `n_d` pairs in general position), whatever the starting model.
#[test]
fn mimo_shock_secant_reproduces_every_pair_of_the_level() {
    let mut rng = Rng::new(3);
    let (lines, n_c, n_d) = (16, 3, 3);
    let mut prior = MatrixStack::zeros(lines, n_c, n_d);
    for v in prior.data_mut() {
        *v = C64::new(rng.normal(), rng.normal());
    }
    let mut truth = prior.clone();
    for v in truth.data_mut() {
        *v += C64::new(0.3 * rng.normal(), 0.3 * rng.normal());
    }
    let mut p = MultiSecantModel::new(
        prior,
        vec![true; lines],
        SecantOptions {
            gate: 0.0,
            ..Default::default()
        },
    );
    let mut pairs = Vec::new();
    for _ in 0..3 {
        let d = random_spectra(n_d, lines, &mut rng);
        let y = apply(&truth, &d);
        p.observe(&Observation {
            level_db: 0.0,
            drives: &d,
            responses: &y,
        })
        .unwrap();
        pairs.push((d, y));
    }
    for (d, y) in &pairs {
        let got = apply(p.current(), d);
        for (gr, yr) in got.iter().zip(y) {
            for (g, w) in gr.iter().zip(yr) {
                assert!((g - w).norm() < 1e-6 * w.norm().max(1.0));
            }
        }
    }
}

/// One exact pulse at a new level moves every tracked frequency and damping
/// to the plant's (the update is exact on a quasi-linear plant).
#[test]
fn mimo_shock_track_recovers_the_level_from_one_pulse() {
    let plant = LevelPlant::four_by_four_damped(0.08, 1.0, 0.02);
    let (setup, modal) = case(&plant, true, false);
    let h = setup.plant.frf.clone();
    let mut p = PoleTrackModel::new(
        modal,
        h.clone(),
        band(),
        FS_HZ,
        TrackOptions::default(),
        Box::new(FrozenModel::new(h, band())),
    )
    .unwrap();
    let mut law = ShapeLaw::new(setup.clone(), ShapeParams::default()).unwrap();
    let out = law.seed(-3.0).unwrap();
    let s = 10f64.powf(-3.0 / 20.0);
    let y = plant.respond(&out.drives, s, FS_HZ).unwrap();
    let d = hertzforge_mimo_shock_law_math::spectrum::rows_rfft(&out.drives, s).unwrap();
    let yy = hertzforge_mimo_shock_law_math::spectrum::rows_rfft(&y, s).unwrap();
    p.observe(&Observation {
        level_db: -3.0,
        drives: &d,
        responses: &yy,
    })
    .unwrap();
    let want = plant.modal_truth(s);
    for (a, b) in p.modal().modes.iter().zip(&want.modes) {
        assert!(
            (a.fn_hz / b.fn_hz - 1.0).abs() < 1e-6,
            "{} {}",
            a.fn_hz,
            b.fn_hz
        );
        assert!(
            (a.zeta / b.zeta - 1.0).abs() < 1e-4,
            "{} {}",
            a.zeta,
            b.zeta
        );
    }
}
