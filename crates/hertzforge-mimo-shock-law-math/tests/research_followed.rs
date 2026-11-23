//! `followed` and `followed-hold` (`followed.rs`): `followed` equals the
//! research bridge's assembly, and each of followed-hold's pieces on and off.
//! Release: `cargo test --release -p hertzforge-mimo-shock-law-math --test research_followed`.

use hertzforge_mimo_shock_law_math::followed::{
    followed_warp_options, margin_at, probe_scale_db, FollowedLaw, FollowedOptions,
};
use hertzforge_mimo_shock_law_math::provider::{ModelProvider, Observation};
use hertzforge_mimo_shock_law_math::reference_plant::{FS_HZ, RECORD_LEN};
use hertzforge_mimo_shock_law_math::research_plant::{research_setup, LevelPlant};
use hertzforge_mimo_shock_law_math::shape::{
    CarryGain, ShapeExtras, ShapeLaw, ShapeParams, ShapeSafety,
};
use hertzforge_mimo_shock_law_math::softening::{Follow, SofteningFamily, SofteningPlan};
use hertzforge_mimo_shock_law_math::spectrum::rows_rfft;
use hertzforge_mimo_shock_law_math::warp::WarpModel;
use hertzforge_mimo_shock_law_math::{drive_mask, level_scale, LawSteps, PulseInputs, Setup, C64};

const LADDER: [f64; 11] = [
    -40.0, -34.0, -12.0, -9.0, -6.0, -3.0, 0.0, 0.0, 0.0, 0.0, 0.0,
];

fn softening() -> LevelPlant {
    LevelPlant::four_by_four_damped(0.08, 0.0, 0.02)
}

fn setup_on(plant: &LevelPlant) -> Setup {
    research_setup(plant.frf(level_scale(-40.0), FS_HZ, RECORD_LEN))
}

/// The research bridge's `followed` (`bridge/src/lib.rs`, `build_law`, kind
/// `followed`, copied without change of arithmetic; its `Logged` wrapper only
/// forwards).
fn bridge_followed(ms: &Setup) -> ShapeLaw {
    let base = ms.plant.frf.clone();
    let mask = drive_mask(ms);
    let p = SofteningPlan {
        follow: Follow::Levels {
            narrow: 1.0,
            within_level: false,
        },
        ladder_start_db: Some(-12.0),
        ..SofteningPlan::default()
    };
    let w = WarpModel::new(
        base.clone(),
        mask.clone(),
        ms.fs_hz,
        followed_warp_options(ms.fs_hz),
    )
    .unwrap();
    let f = SofteningFamily::new(w, base, mask, rows_rfft(&ms.reference, 1.0).unwrap(), p).unwrap();
    ShapeLaw::with_options(
        ms.clone(),
        ShapeParams::default(),
        ShapeExtras {
            provider: Some(Box::new(f)),
            safety: ShapeSafety {
                carry: CarryGain::Keep,
                ..ShapeSafety::off()
            },
            robust_first: None,
        },
    )
    .unwrap()
}

/// Closed loop over `LADDER` (the planned levels, no engine), every pulse's
/// drives and the law's values after it.
type Pulse = (Vec<Vec<f64>>, Vec<(&'static str, f64)>);

fn run(law: &mut dyn LawSteps, plant: &LevelPlant) -> Vec<Pulse> {
    let mut d = law.seed(LADDER[0]).unwrap();
    let mut out = vec![(d.drives.clone(), d.values.clone())];
    for (k, w) in LADDER.windows(2).enumerate() {
        let s = level_scale(w[0]);
        let y = plant.respond(&d.drives, s, FS_HZ).unwrap();
        d = law
            .iterate(&PulseInputs {
                pulse_index: k,
                level_db: w[0],
                next_level_db: w[1],
                drives: d.drives.clone(),
                responses: y,
                srs: None,
            })
            .unwrap();
        out.push((d.drives.clone(), d.values.clone()));
    }
    out
}

fn rel(a: &[Vec<f64>], b: &[Vec<f64>]) -> f64 {
    let m = b
        .iter()
        .flatten()
        .fold(0.0f64, |x, v| x.max(v.abs()))
        .max(1e-300);
    a.iter()
        .flatten()
        .zip(b.iter().flatten())
        .fold(0.0f64, |x, (p, q)| x.max((p - q).abs()))
        / m
}

fn value(v: &[(&'static str, f64)], k: &str) -> Option<f64> {
    v.iter().find(|(n, _)| *n == k).map(|(_, x)| *x)
}

#[test]
fn mimo_shock_followed_equals_the_bridge_assembly() {
    let plant = softening();
    let s = setup_on(&plant);
    let a = run(&mut FollowedLaw::followed(s.clone()).unwrap(), &plant);
    let b = run(&mut bridge_followed(&s), &plant);
    let worst = a
        .iter()
        .zip(&b)
        .map(|(x, y)| rel(&x.0, &y.0))
        .fold(0.0, f64::max);
    println!("followed against the bridge's assembly: largest drive difference {worst:.3e}");
    assert!(worst <= 1e-12, "{worst}");
}

#[test]
fn mimo_shock_followed_hold_with_its_pieces_off_is_followed_without_the_ceiling() {
    let plant = softening();
    let s = setup_on(&plant);
    let off = FollowedOptions {
        resolved_power_gate: None,
        hold_db: None,
        probe_check: None,
        ..FollowedOptions::followed_hold()
    };
    let wide = FollowedOptions {
        ceiling: f64::INFINITY,
        ..FollowedOptions::followed()
    };
    let a = run(&mut FollowedLaw::new(s.clone(), off).unwrap(), &plant);
    let b = run(&mut FollowedLaw::new(s, wide).unwrap(), &plant);
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.0, y.0);
    }
}

#[test]
fn mimo_shock_margin_and_probe_scale() {
    assert_eq!(margin_at(0.0, 1.0, 2.0, 1.0), 0.0);
    assert!((margin_at(3.0, 1.0, 1.0, 1.0) - 3.0).abs() < 1e-12);
    assert!(margin_at(3.0, 1.0, 2.0, 1.0) > margin_at(3.0, 1.0, 1.0, 1.0));
    assert_eq!(probe_scale_db(1.0, 3.0, 0.01, &[], 1.0), 0.0);
    assert!((probe_scale_db(5.0, 3.0, 0.01, &[], 1.0) + 2.0).abs() < 1e-9);
    assert_eq!(probe_scale_db(60.0, 3.0, 0.01, &[], 1.0), -40.0);
    // With a measured error the scale also brings the margin under the limit.
    let eps = [(0.01, 2.0)];
    let k = probe_scale_db(2.0, 3.0, 0.01, &eps, 1.0);
    let a = 0.01 * 10f64.powf(k / 20.0);
    assert!(k < 0.0);
    assert!((2.0 + k + margin_at(2.0, 0.01, a, 1.0) - 3.0).abs() < 1e-9);
}

/// The power bound raises the assumed drop where both levels' drops pass
/// the gate, and a gate no drop reaches changes nothing.
#[test]
fn mimo_shock_followed_power_bound_on_and_off() {
    let plant = softening();
    let s = setup_on(&plant);
    let fam = |gate: Option<f64>| {
        let base = s.plant.frf.clone();
        let mask = drive_mask(&s);
        let w = WarpModel::new(
            base.clone(),
            mask.clone(),
            s.fs_hz,
            followed_warp_options(s.fs_hz),
        )
        .unwrap();
        let o = FollowedOptions {
            ceiling: f64::INFINITY,
            ..FollowedOptions::followed()
        };
        let f = SofteningFamily::new(
            w,
            base,
            mask,
            rows_rfft(&s.reference, 1.0).unwrap(),
            hertzforge_mimo_shock_law_math::followed::followed_plan(&o),
        )
        .unwrap();
        match gate {
            Some(g) => f.with_resolved_power(g),
            None => f,
        }
    };
    let mut seed = FollowedLaw::followed(s.clone()).unwrap();
    let d = seed.seed(0.0).unwrap().drives;
    let mut fams = [fam(None), fam(Some(0.005)), fam(Some(1.0))];
    for level in [-6.0, -3.0] {
        let sc = level_scale(level);
        let dl: Vec<Vec<f64>> = d
            .iter()
            .map(|r| r.iter().map(|v| v * sc).collect())
            .collect();
        let y = plant.respond(&dl, sc, FS_HZ).unwrap();
        let (ds, ys): (Vec<Vec<C64>>, Vec<Vec<C64>>) =
            (rows_rfft(&dl, sc).unwrap(), rows_rfft(&y, sc).unwrap());
        for f in fams.iter_mut() {
            f.observe(&Observation {
                level_db: level,
                drives: &ds,
                responses: &ys,
            })
            .unwrap();
        }
    }
    for f in fams.iter_mut() {
        f.model_for(0.0).unwrap();
    }
    let (a, b, c) = (fams[0].assumed(), fams[1].assumed(), fams[2].assumed());
    let fit = fams[0].levels().last().unwrap().fit.drops.clone();
    println!("drops at -3 dB {fit:?}; assumed for 0 dB: measured {a:?}, bound {b:?}");
    assert_eq!(a, c);
    assert!(a.iter().zip(b).all(|(x, y)| y >= x));
    assert!(a.iter().zip(b).any(|(x, y)| y > x));
}

/// On a repeat pulse within 3 dB the hold plays the same drive again; with
/// the hold off the drive changes.
#[test]
fn mimo_shock_followed_hold_on_and_off() {
    let plant = LevelPlant::four_by_four_damped(0.0, 0.0, 0.02);
    let s = setup_on(&plant);
    let on = FollowedOptions {
        hold_db: Some(3.0),
        ..FollowedOptions::followed()
    };
    let a = run(&mut FollowedLaw::new(s.clone(), on).unwrap(), &plant);
    let b = run(&mut FollowedLaw::followed(s).unwrap(), &plant);
    let mut held = 0;
    for k in 1..a.len() {
        if value(&a[k].1, "held") == Some(1.0) {
            held += 1;
            assert!(rel(&a[k].0, &a[k - 1].0) < 1e-12, "pulse {k}");
            assert!(LADDER[k] == LADDER[k - 1]);
        }
    }
    assert!(held > 0);
    let changed = (1..b.len())
        .filter(|&k| LADDER[k] == LADDER[k - 1])
        .any(|k| rel(&b[k].0, &b[k - 1].0) > 1e-9);
    assert!(changed);
}

/// A model at half the plant's gain: the first probe measures about +6 dB,
/// so the second is predicted over and scaled down; with the check off it is
/// the same drive unscaled.
#[test]
fn mimo_shock_followed_probe_check_on_and_off() {
    let plant = LevelPlant::four_by_four_damped(0.0, 0.0, 0.02);
    let mut s = setup_on(&plant);
    s.plant.frf.data_mut().iter_mut().for_each(|v| *v *= 0.5);
    let check = FollowedOptions::followed_hold().probe_check;
    let on = FollowedOptions {
        probe_check: check,
        ..FollowedOptions::followed()
    };
    let mut law = FollowedLaw::new(s.clone(), on).unwrap();
    let a = run(&mut law, &plant);
    let b = run(&mut FollowedLaw::followed(s).unwrap(), &plant);
    let c = law.probe_checks();
    println!("probe checks (level, scale, predicted, measured): {c:?}");
    assert_eq!(c.len(), 2);
    assert_eq!(c[0].1, 0.0);
    assert!(c[0].3 > 5.0);
    let k = c[1].1;
    assert!(k < -1.0);
    assert!(c[1].2 < 3.0);
    assert_eq!(a[0].0, b[0].0);
    let g = 10f64.powf(k / 20.0);
    let scaled: Vec<Vec<f64>> = b[1]
        .0
        .iter()
        .map(|r| r.iter().map(|v| v * g).collect())
        .collect();
    assert!(rel(&a[1].0, &scaled) < 1e-12);
}
