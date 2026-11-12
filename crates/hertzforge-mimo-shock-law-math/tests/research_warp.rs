//! The warp follower and the choice between followers: the requirements W1,
//! W4 (in part), W5, W7, W8 and W9 of the research record, fixed before the
//! code existed.

use hertzforge_mimo_shock_law_math::choice::{ChoiceModel, ChoiceOptions};
use hertzforge_mimo_shock_law_math::inverse::{InverseExtras, InverseLaw, InverseParams};
use hertzforge_mimo_shock_law_math::modal::{fit_modal, FitOptions, ModalModel};
use hertzforge_mimo_shock_law_math::provider::{
    FrozenModel, ModelProvider, MultiSecantModel, Observation, PoleTrackModel, SecantOptions,
    TrackOptions,
};
use hertzforge_mimo_shock_law_math::reference_plant::{
    reference_shock, Rng, FS_HZ, LADDER_DB, RECORD_LEN,
};
use hertzforge_mimo_shock_law_math::research_plant::{
    identify_h1_model, research_setup_signed, run_research_ladder, LevelMode, LevelPlant,
    ResearchRow, SpreadPlant,
};
use hertzforge_mimo_shock_law_math::shape::{ShapeExtras, ShapeLaw, ShapeParams, ShapeSafety};
use hertzforge_mimo_shock_law_math::spectrum::{apply, rfft};
use hertzforge_mimo_shock_law_math::warp::{WarpModel, WarpOptions};
use hertzforge_mimo_shock_law_math::{level_scale, LawSteps, MatrixStack, Setup, C64};
use std::time::Instant;

const S_ID: f64 = 0.1;
const SEED: u64 = 20_261_111;

fn band() -> Vec<bool> {
    let df = FS_HZ / RECORD_LEN as f64;
    (0..RECORD_LEN / 2 + 1)
        .map(|k| {
            let f = k as f64 * df;
            (80.0..=2400.0).contains(&f) && k > 0
        })
        .collect()
}

fn warp(h: &MatrixStack, gate: Option<f64>) -> WarpModel {
    WarpModel::new(
        h.clone(),
        band(),
        FS_HZ,
        WarpOptions {
            identification_scale: Some(S_ID),
            gate,
            ..Default::default()
        },
    )
    .expect("warp")
}

/// One pulse's spectra: the regularized inverse of `hid` applied to the
/// reference on every control (signs `pol`), played at `level_db` on `h`,
/// with absolute noise of `noise` times the reference's rms (seeded),
/// normalized to full level.
fn pulse(
    hid: &MatrixStack,
    h: &MatrixStack,
    level_db: f64,
    pol: &[f64],
    noise: f64,
    seed: u64,
) -> (Vec<Vec<C64>>, Vec<Vec<C64>>) {
    let g = hertzforge_mimo_shock_law_math::model::regularized_inverse(
        hid,
        &vec![true; hid.lines()],
        1e-3,
    )
    .unwrap();
    let r = reference_shock(FS_HZ, RECORD_LEN);
    let rr = rfft(&r).unwrap();
    let refs: Vec<Vec<C64>> = pol
        .iter()
        .map(|p| rr.iter().map(|v| v * *p).collect())
        .collect();
    let d = apply(&g, &refs);
    let s = level_scale(level_db);
    let mut y = apply(h, &d);
    if noise > 0.0 {
        let rms = (r.iter().map(|v| v * v).sum::<f64>() / r.len() as f64).sqrt();
        let mut rng = Rng::new(seed);
        for row in y.iter_mut() {
            let mut t = hertzforge_mimo_shock_law_math::spectrum::irfft(row, RECORD_LEN).unwrap();
            for v in t.iter_mut() {
                *v += noise * rms * rng.normal() / s;
            }
            *row = rfft(&t).unwrap();
        }
    }
    (d, y)
}

#[test]
fn mimo_shock_w7a_the_decay_never_goes_negative() {
    // A plant whose damping falls steeply with level: the fit wants a
    // negative decay (a window growing in time), which must be refused.
    let p = LevelPlant::four_by_four_damped(0.0, -0.9, 0.02);
    let hid = p.frf(S_ID, FS_HZ, RECORD_LEN);
    let mut w = warp(&hid, None);
    for level in [-3.0, 0.0] {
        let (d, y) = pulse(
            &hid,
            &p.frf(level_scale(level), FS_HZ, RECORD_LEN),
            level,
            &[1.0, -1.0, 1.0, -1.0],
            0.0,
            1,
        );
        w.observe(&Observation {
            level_db: level,
            drives: &d,
            responses: &y,
        })
        .unwrap();
        w.model_for(3.0).unwrap();
        for (s, x) in w.history() {
            let nk = x.len() / 2;
            let least = x[nk..].iter().fold(f64::INFINITY, |a, &b| a.min(b));
            assert!(least >= 0.0, "a negative decay {least} stored at scale {s}");
        }
        let x = w.parameters();
        let nk = x.len() / 2;
        let least = x[nk..].iter().fold(f64::INFINITY, |a, &b| a.min(b));
        assert!(least >= 0.0, "a negative decay {least} predicted");
    }
    // A negative decay handed to the model reads the unwindowed copy.
    let nk = w.parameters().len() / 2;
    let mut neg = vec![0.0; 2 * nk];
    neg[nk..].iter_mut().for_each(|v| *v = -300.0);
    let zero = vec![0.0; 2 * nk];
    let (a, b) = (w.model_at(&neg), w.model_at(&zero));
    assert_eq!(a.data(), b.data());
}

#[test]
fn mimo_shock_w7b_the_regularized_cost_never_rises() {
    // A large shift at once: the identification at -20 dB, the pulse at full
    // level on a plant that drops 15% (an undamped Gauss-Newton step
    // overshoots here).
    let p = LevelPlant::four_by_four_damped(0.15, 1.0, 0.02);
    let hid = p.frf(S_ID, FS_HZ, RECORD_LEN);
    let mut w = warp(&hid, None);
    let (d, y) = pulse(
        &hid,
        &p.frf(1.0, FS_HZ, RECORD_LEN),
        0.0,
        &[1.0, -1.0, 1.0, -1.0],
        0.0,
        1,
    );
    w.observe(&Observation {
        level_db: 0.0,
        drives: &d,
        responses: &y,
    })
    .unwrap();
    let c = w.last_costs().to_vec();
    assert!(c.len() >= 2, "no step was taken: {c:?}");
    for k in 1..c.len() {
        assert!(c[k] <= c[k - 1], "the cost rose at step {k}: {c:?}");
    }
    assert!(
        c[c.len() - 1] < 0.5 * c[0],
        "the fit did not progress: {c:?}"
    );
}

/// How far any warp moves a plant that is a constant coupling with control
/// delays (no resonance, so a warp should leave it unchanged).
fn delay_only_warp_error(delays_s: Vec<f64>) -> f64 {
    let p = LevelPlant {
        n_c: 3,
        n_d: 2,
        quasi_static: vec![1.0, 0.3, -0.2, 0.8, 0.5, 0.4],
        modes: Vec::<LevelMode>::new(),
        delta: 0.0,
        kappa: 0.0,
        delays_s,
        drop_law: hertzforge_mimo_shock_law_math::research_plant::DropLaw::Linear,
    };
    let hid = p.frf(S_ID, FS_HZ, RECORD_LEN);
    let w = warp(&hid, None);
    for (r, (got, want)) in w.delays().iter().zip(&p.delays_s).enumerate() {
        assert!(
            (got - want).abs() < 1e-9,
            "control {r}: delay {got} for {want}"
        );
    }
    let nk = w.parameters().len() / 2;
    let x: Vec<f64> = (0..2 * nk)
        .map(|k| {
            if k < nk {
                -0.1 + 0.004 * k as f64
            } else {
                150.0 + 5.0 * (k - nk) as f64
            }
        })
        .collect();
    let m = w.model_at(&x);
    let (mut num, mut den) = (0.0, 0.0);
    for (f, &on) in band().iter().enumerate() {
        if on {
            for (a, b) in m.line(f).iter().zip(hid.line(f)) {
                num += (a - b).norm_sqr();
                den += b.norm_sqr();
            }
        }
    }
    (num / den).sqrt()
}

#[test]
fn mimo_shock_w7c_delays_are_removed_before_warping() {
    // Delays of whole samples: the warp leaves the plant exactly as it was.
    let whole = delay_only_warp_error(vec![1.0 / FS_HZ, 3.0 / FS_HZ, 6.0 / FS_HZ]);
    // Fractional delays: a real record's Nyquist line cannot carry them, so
    // the delay-free impulse response keeps a component of order 1 / N that
    // the window changes; the plant moves by that much and no more.
    let frac = delay_only_warp_error(vec![0.05e-3, 0.17e-3, 0.31e-3]);
    println!("delay-only plant moved by {whole:.2e} (whole samples), {frac:.2e} (fractional)");
    assert!(whole < 1e-9, "{whole}");
    assert!(frac < 1e-4, "{frac}");
}

#[test]
fn mimo_shock_w8_the_gate_refuses_a_fit_to_noise() {
    // The plant does not move with level; the pulse carries noise of 30% of
    // the reference's rms. The ungated fit moves; the gated one keeps the
    // prediction.
    let p = LevelPlant::four_by_four_damped(0.0, 0.0, 0.02);
    let hid = p.frf(S_ID, FS_HZ, RECORD_LEN);
    let (d, y) = pulse(
        &hid,
        &p.frf(level_scale(-12.0), FS_HZ, RECORD_LEN),
        -12.0,
        &[1.0, -1.0, 1.0, -1.0],
        0.30,
        9,
    );
    let obs = Observation {
        level_db: -12.0,
        drives: &d,
        responses: &y,
    };
    let moved = |w: &WarpModel| -> f64 {
        let x = &w
            .history()
            .iter()
            .find(|h| h.0 > 0.2)
            .expect("level point")
            .1;
        let nk = x.len() / 2;
        x.iter()
            .enumerate()
            .map(|(k, v)| {
                if k < nk {
                    (v / 0.05).abs()
                } else {
                    (v / 100.0).abs()
                }
            })
            .fold(0.0f64, f64::max)
    };
    let mut ungated = warp(&hid, None);
    ungated.observe(&obs).unwrap();
    let mut gated = warp(&hid, Some(2.0));
    gated.observe(&obs).unwrap();
    let (refused, dof) = gated.gate_counts();
    println!(
        "ungated move {:.3e} (scaled), gated move {:.3e}, refused {refused}, dof {dof:.1}",
        moved(&ungated),
        moved(&gated)
    );
    assert!(moved(&ungated) > 1e-3, "the ungated fit did not move");
    assert_eq!(refused, 1);
    assert_eq!(moved(&gated), 0.0);
    // And on a plant that moved, clean, the gate accepts.
    let p8 = LevelPlant::four_by_four_damped(0.08, 0.0, 0.02);
    let hid8 = p8.frf(S_ID, FS_HZ, RECORD_LEN);
    let (d, y) = pulse(
        &hid8,
        &p8.frf(level_scale(-12.0), FS_HZ, RECORD_LEN),
        -12.0,
        &[1.0; 4],
        0.0,
        9,
    );
    let mut g8 = warp(&hid8, Some(2.0));
    g8.observe(&Observation {
        level_db: -12.0,
        drives: &d,
        responses: &y,
    })
    .unwrap();
    assert_eq!(g8.gate_counts().0, 0, "a real shift was refused");
}

#[test]
fn mimo_shock_w4_the_choice_holds_on_noise() {
    // Two followers whose models differ by far less than the noise: with the
    // noise test the choice never moves; on a plain ratio of residuals with
    // no margin it flips on the noise.
    let p = LevelPlant::four_by_four_damped(0.0, 0.0, 0.02);
    let hid = p.frf(S_ID, FS_HZ, RECORD_LEN);
    let mut other = hid.clone();
    for v in other.data_mut() {
        *v *= C64::new(1.0, 0.002);
    }
    let run = |opts: ChoiceOptions| -> usize {
        let mut c = ChoiceModel::new(
            Box::new(FrozenModel::new(hid.clone(), band())),
            Box::new(FrozenModel::new(other.clone(), band())),
            band(),
            FS_HZ,
            opts,
        )
        .unwrap();
        let log = c.log();
        for k in 0..12u64 {
            let (d, y) = pulse(
                &hid,
                &p.frf(level_scale(-12.0), FS_HZ, RECORD_LEN),
                -12.0,
                &[1.0, -1.0, 1.0, -1.0],
                0.30,
                100 + k,
            );
            c.observe(&Observation {
                level_db: -12.0,
                drives: &d,
                responses: &y,
            })
            .unwrap();
            c.model_for(-12.0).unwrap();
        }
        let l = log.lock().unwrap();
        l.windows(2).filter(|w| w[0].chosen != w[1].chosen).count()
            + usize::from(l[0].chosen[0] != 0)
    };
    let plain = run(ChoiceOptions {
        ratio: 1.0,
        z: 0.0,
        ..Default::default()
    });
    let held = run(ChoiceOptions::default());
    println!("moves: plain ratio {plain}, with the noise test {held}");
    assert!(plain >= 2, "the plain rule did not flip ({plain})");
    assert_eq!(held, 0);
}

// ---------------------------------------------------------------- ladders

fn h1_case(p: &LevelPlant, mixed: bool) -> (Setup, Option<ModalModel>) {
    let truth = p.modal_truth(S_ID);
    let h = identify_h1_model(&truth, 16, 0.05, 7).unwrap();
    let pol: Vec<f64> = (0..p.n_c)
        .map(|c| if mixed && c % 2 == 1 { -1.0 } else { 1.0 })
        .collect();
    let fit = fit_modal(
        &h,
        FS_HZ,
        &FitOptions {
            n_modes: p.modes.len(),
            ..Default::default()
        },
    )
    .ok()
    .map(|f| f.model);
    (research_setup_signed(h, &pol), fit)
}

fn warp_provider(setup: &Setup) -> Box<dyn ModelProvider> {
    Box::new(warp(&setup.plant.frf, None))
}

fn track_provider(setup: &Setup, modal: &ModalModel) -> Box<dyn ModelProvider> {
    let h = setup.plant.frf.clone();
    Box::new(
        PoleTrackModel::new(
            modal.clone(),
            h.clone(),
            band(),
            FS_HZ,
            TrackOptions {
                secant: Some(SecantOptions::default()),
                ..Default::default()
            },
            Box::new(MultiSecantModel::new(h, band(), SecantOptions::default())),
        )
        .unwrap(),
    )
}

fn shape_with(setup: &Setup, provider: Box<dyn ModelProvider>) -> ShapeLaw {
    ShapeLaw::with_options(
        setup.clone(),
        ShapeParams::default(),
        ShapeExtras {
            provider: Some(provider),
            safety: ShapeSafety::off(),
            robust_first: None,
        },
    )
    .unwrap()
}

fn ladder(law: &mut dyn LawSteps, setup: &Setup, p: &LevelPlant, noise: f64) -> Vec<ResearchRow> {
    let mut resp = |d: &[Vec<f64>], s: f64| p.respond(d, s, FS_HZ);
    run_research_ladder(law, setup, &mut resp, &LADDER_DB, noise, SEED, None).unwrap()
}

#[test]
fn mimo_shock_w1_warp_with_secant_on_the_shifted_plants() {
    for (name, p) in [
        ("8% fixed", LevelPlant::four_by_four_damped(0.08, 0.0, 0.02)),
        ("5% x2", LevelPlant::four_by_four_damped(0.05, 1.0, 0.02)),
        ("5% fixed", LevelPlant::four_by_four_damped(0.05, 0.0, 0.02)),
    ] {
        let (setup, _) = h1_case(&p, true);
        for (noise, p5_max) in [(0.0, 1.5), (0.30, 2.5)] {
            let mut law = shape_with(&setup, warp_provider(&setup));
            let rows = ladder(&mut law, &setup, &p, noise);
            println!(
                "W1 {name} noise {noise}: p5 over {:+.2} p8 {:.2} p14 {:.2}",
                rows[4].over_db, rows[7].worst_db, rows[13].worst_db
            );
            assert!(
                rows[4].over_db < p5_max,
                "{name} noise {noise}: p5 {:+.2}",
                rows[4].over_db
            );
            assert!(
                rows[7].worst_db < 1.0,
                "{name} noise {noise}: p8 {:.2}",
                rows[7].worst_db
            );
        }
    }
}

#[test]
fn mimo_shock_w4_the_choice_is_near_the_better_follower() {
    for (name, p) in [
        ("8% fixed", LevelPlant::four_by_four_damped(0.08, 0.0, 0.02)),
        ("5% x2", LevelPlant::four_by_four_damped(0.05, 1.0, 0.02)),
    ] {
        let (setup, modal) = h1_case(&p, true);
        let modal = modal.expect("fit");
        for noise in [0.0, 0.30] {
            let mut lw = shape_with(&setup, warp_provider(&setup));
            let w = ladder(&mut lw, &setup, &p, noise);
            let mut lt = shape_with(&setup, track_provider(&setup, &modal));
            let t = ladder(&mut lt, &setup, &p, noise);
            let choice = ChoiceModel::new(
                warp_provider(&setup),
                track_provider(&setup, &modal),
                band(),
                FS_HZ,
                ChoiceOptions::default(),
            )
            .unwrap();
            let mut lc = shape_with(&setup, Box::new(choice));
            let c = ladder(&mut lc, &setup, &p, noise);
            let p5 = w[4].over_db.min(t[4].over_db);
            let p8 = w[7].worst_db.min(t[7].worst_db);
            println!(
                "W4 {name} noise {noise}: p5 warp {:+.2} track {:+.2} choice {:+.2}; p8 {:.2} {:.2} {:.2}",
                w[4].over_db, t[4].over_db, c[4].over_db, w[7].worst_db, t[7].worst_db, c[7].worst_db
            );
            assert!(c[4].over_db <= p5 + 0.5, "{name} {noise}: p5");
            assert!(c[7].worst_db <= p8 + 0.5, "{name} {noise}: p8");
        }
    }
}

#[test]
fn mimo_shock_w9_inverse_level_step_on_the_warp() {
    let p = LevelPlant::four_by_four_damped(0.08, 0.0, 0.02);
    let (setup, _) = h1_case(&p, true);
    let mut law = InverseLaw::with_options(
        setup.clone(),
        InverseParams {
            watchdog: false,
            ..Default::default()
        },
        InverseExtras {
            provider: Some(warp_provider(&setup)),
            robust_first: None,
            predicted_step: true,
        },
    )
    .unwrap();
    let rows = ladder(&mut law, &setup, &p, 0.0);
    println!(
        "W9 p5 over {:+.2}, p8 {:.2}",
        rows[4].over_db, rows[7].worst_db
    );
    assert!(rows[4].over_db < 1.0, "{:+.2}", rows[4].over_db);
}

#[test]
fn mimo_shock_w5_warp_cost_per_pulse() {
    let p = LevelPlant::four_by_four_damped(0.08, 0.0, 0.02);
    let hid = p.frf(S_ID, FS_HZ, RECORD_LEN);
    let mut w = warp(&hid, None);
    let mut worst = 0.0f64;
    for (level, next) in [
        (-12.0, -12.0),
        (-12.0, -12.0),
        (-12.0, -9.0),
        (0.0, 0.0),
        (0.0, 0.0),
    ] {
        let (d, y) = pulse(
            &hid,
            &p.frf(level_scale(level), FS_HZ, RECORD_LEN),
            level,
            &[1.0; 4],
            0.0,
            1,
        );
        let t = Instant::now();
        w.observe(&Observation {
            level_db: level,
            drives: &d,
            responses: &y,
        })
        .unwrap();
        w.model_for(next).unwrap();
        worst = worst.max(t.elapsed().as_secs_f64());
    }
    println!(
        "W5 4 x 4 worst observe and next model {:.0} ms",
        1e3 * worst
    );
    assert!(worst < 0.25, "{worst} s");
}

#[test]
fn mimo_shock_spread_plant_matches_the_level_plant() {
    // The mixed-movement plant with every mode moving alike is the level
    // plant.
    let mut sp = SpreadPlant::four_by_four_mixed();
    for m in sp.modes.iter_mut() {
        m.drop = 0.08;
        m.growth = 0.5;
    }
    let lp = LevelPlant::four_by_four_damped(0.08, 0.5, 0.02);
    let a = sp.frf(0.7, FS_HZ, 1024);
    let b = lp.frf(0.7, FS_HZ, 1024);
    let worst = a.data().iter().zip(b.data()).fold(0.0f64, |m, (x, y)| {
        m.max((x - y).norm() / y.norm().max(1e-12))
    });
    assert!(worst < 1e-12, "{worst}");
}
