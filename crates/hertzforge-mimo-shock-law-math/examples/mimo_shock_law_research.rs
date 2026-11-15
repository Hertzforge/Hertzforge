//! The MIMO Shock law-research study: each model provider and option beside
//! its baseline, on the research plants, clean and noisy, with the true modal
//! model and with one fitted from a realistic identification. Fixed seeds, no
//! file output; it prints tables.
//!
//! ```text
//! cargo run --release -p hertzforge-mimo-shock-law-math --features reference-plant \
//!     --example mimo_shock_law_research -- <part>
//! ```
//!
//! Parts: `main` (the 4 x 4 table), `excite` (which modes each target set
//! excites, and the seed), `stress` (wrong residues, moving shapes, quadratic
//! drop, close modes, the intra-pulse plant, the 2 x 3), `sparse`, `safety`
//! (100 pulses, clipped drive), `secant` (the gate under noise), `cost`.
//!
//! Columns: `p1+` the seed's signed over-test; `p5` worst error of the first
//! full-level pulse with its signed over-test in brackets; `p6`, `p8`, `p14`
//! worst error; `grow` largest drive growth over the seed; `C` the largest
//! accumulated gain of `shape`, dB; `res5`, `res8` the model's prediction
//! residual on p5 and p8; `gate` pulses handed to the fallback.

use hertzforge_mimo_shock_law_math::inverse::{InverseExtras, InverseLaw, InverseParams};
use hertzforge_mimo_shock_law_math::modal::{fit_modal, FitOptions, ModalModel};
use hertzforge_mimo_shock_law_math::provider::{
    FrozenModel, ModelProvider, MultiSecantModel, PoleTrackModel, RefreshModel, SecantOptions,
    Sparse, TrackOptions,
};
use hertzforge_mimo_shock_law_math::reference_plant::{ModalPlant, FS_HZ, LADDER_DB, RECORD_LEN};
use hertzforge_mimo_shock_law_math::research_plant::{
    identify_h1, research_setup_signed, run_research_ladder, Ceiling, DropLaw, IntraPulsePlant,
    LevelPlant, ResearchRow, Respond,
};
use hertzforge_mimo_shock_law_math::shape::{ShapeExtras, ShapeLaw, ShapeParams, ShapeSafety};
use hertzforge_mimo_shock_law_math::spectrum::{apply, rows_irfft, rows_rfft};
use hertzforge_mimo_shock_law_math::srs::{error_db, srs_rows};
use hertzforge_mimo_shock_law_math::{LawSteps, MatrixStack, Setup, C64};
use std::f64::consts::TAU;
use std::time::Instant;

const S_ID: f64 = 0.1;
const SEED: u64 = 20_261_111;
const ID_SEED: u64 = 7;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Shape,
    Inverse,
}

#[derive(Clone, Copy, PartialEq)]
enum Prov {
    None,
    Refresh,
    Secant(f64),
    Track {
        extrapolate: bool,
        gate: Option<f64>,
        secant: bool,
        sparse: Sparse,
        refit: bool,
    },
    TrackFromId,
}

#[derive(Clone, Copy)]
struct Spec {
    name: &'static str,
    kind: Kind,
    prov: Prov,
    safety: bool,
    robust: bool,
    predicted_step: bool,
}

const fn track(extrapolate: bool, secant: bool) -> Prov {
    Prov::Track {
        extrapolate,
        gate: Some(1.2),
        secant,
        sparse: Sparse::None,
        refit: false,
    }
}

const fn spec(name: &'static str, kind: Kind, prov: Prov) -> Spec {
    Spec {
        name,
        kind,
        prov,
        safety: false,
        robust: false,
        predicted_step: false,
    }
}

fn main_laws() -> Vec<Spec> {
    vec![
        spec("shape [baseline default]", Kind::Shape, Prov::None),
        spec(
            "inverse+watchdog [baseline default]",
            Kind::Inverse,
            Prov::None,
        ),
        spec("shape + refresh", Kind::Shape, Prov::Refresh),
        spec("shape + secant (ungated)", Kind::Shape, Prov::Secant(0.0)),
        spec(
            "inverse + secant (ungated) [B2]",
            Kind::Inverse,
            Prov::Secant(0.0),
        ),
        spec(
            "inverse + secant (gate 0.2)",
            Kind::Inverse,
            Prov::Secant(0.2),
        ),
        spec("shape + track hold", Kind::Shape, track(false, false)),
        spec("shape + track extrap [B1]", Kind::Shape, track(true, false)),
        spec("inverse + track hold", Kind::Inverse, track(false, false)),
        spec("inverse + track extrap", Kind::Inverse, track(true, false)),
        spec(
            "shape + track extrap + secant",
            Kind::Shape,
            track(true, true),
        ),
        spec(
            "inverse + track extrap + secant",
            Kind::Inverse,
            track(true, true),
        ),
        Spec {
            name: "inverse + track extrap, predicted step",
            predicted_step: true,
            ..spec("", Kind::Inverse, track(true, false))
        },
        Spec {
            name: "inverse + track extrap + secant, pred. step",
            predicted_step: true,
            ..spec("", Kind::Inverse, track(true, true))
        },
        Spec {
            name: "shape + track extrap + B3 safety",
            safety: true,
            ..spec("", Kind::Shape, track(true, false))
        },
        Spec {
            name: "shape + track extrap + B5 weighted",
            robust: true,
            ..spec("", Kind::Shape, track(true, false))
        },
    ]
}

fn fit_band() -> Vec<bool> {
    let df = FS_HZ / RECORD_LEN as f64;
    (0..RECORD_LEN / 2 + 1)
        .map(|k| {
            let f = k as f64 * df;
            (80.0..=2400.0).contains(&f) && k > 0
        })
        .collect()
}

fn build(s: &Spec, setup: &Setup, modal: &ModalModel) -> Box<dyn LawSteps> {
    let band = fit_band();
    let h = setup.plant.frf.clone();
    let provider: Option<Box<dyn ModelProvider>> = match s.prov {
        Prov::None => None,
        Prov::Refresh => Some(Box::new(RefreshModel::new(h, band, 1.0, 1e-6))),
        Prov::Secant(g) => Some(Box::new(MultiSecantModel::new(
            h,
            band,
            SecantOptions {
                gate: g,
                ..Default::default()
            },
        ))),
        Prov::TrackFromId => Some(Box::new(
            PoleTrackModel::new(
                modal.clone(),
                h.clone(),
                band.clone(),
                FS_HZ,
                TrackOptions {
                    identification_scale: Some(S_ID),
                    secant: Some(SecantOptions::default()),
                    ..Default::default()
                },
                Box::new(MultiSecantModel::new(h, band, SecantOptions::default())),
            )
            .expect("track"),
        )),
        Prov::Track {
            extrapolate,
            gate,
            secant,
            sparse,
            refit,
        } => {
            let fallback = Box::new(MultiSecantModel::new(
                h.clone(),
                band.clone(),
                SecantOptions::default(),
            ));
            Some(Box::new(
                PoleTrackModel::new(
                    modal.clone(),
                    h,
                    band,
                    FS_HZ,
                    TrackOptions {
                        extrapolate,
                        gate_ratio: gate,
                        secant: if secant {
                            Some(SecantOptions::default())
                        } else {
                            None
                        },
                        sparse,
                        refit_residues: refit,
                        ..Default::default()
                    },
                    fallback,
                )
                .expect("track"),
            ))
        }
    };
    let robust_first = if s.robust {
        Some(vec![0.01, 0.02, 0.03])
    } else {
        None
    };
    match s.kind {
        Kind::Shape => {
            if provider.is_none() && !s.safety && !s.robust {
                return Box::new(ShapeLaw::new(setup.clone(), ShapeParams::default()).unwrap());
            }
            Box::new(
                ShapeLaw::with_options(
                    setup.clone(),
                    ShapeParams::default(),
                    ShapeExtras {
                        provider,
                        safety: if s.safety {
                            ShapeSafety::recommended()
                        } else {
                            ShapeSafety::off()
                        },
                        robust_first,
                    },
                )
                .unwrap(),
            )
        }
        Kind::Inverse => {
            // The secant and tracking laws run at gain 1 with the watchdog off
            // (the audit's B1 and B2); the baseline keeps its watchdog.
            let params = InverseParams {
                watchdog: s.prov == Prov::None,
                ..Default::default()
            };
            if provider.is_none() {
                return Box::new(InverseLaw::new(setup.clone(), params).unwrap());
            }
            Box::new(
                InverseLaw::with_options(
                    setup.clone(),
                    params,
                    InverseExtras {
                        provider,
                        robust_first,
                        predicted_step: s.predicted_step,
                    },
                )
                .unwrap(),
            )
        }
    }
}

fn polarity(n_c: usize, mixed: bool) -> Vec<f64> {
    (0..n_c)
        .map(|c| if mixed && c % 2 == 1 { -1.0 } else { 1.0 })
        .collect()
}

/// A model case: the setup's plant model and the modal model pole tracking
/// starts from.
struct ModelCase {
    setup: Setup,
    modal: ModalModel,
}

fn model_case(plant: &LevelPlant, mixed: bool, fitted: bool, n_modes: usize) -> ModelCase {
    let pol = polarity(plant.n_c, mixed);
    if fitted {
        let h = identify_h1(plant, S_ID, 16, 0.05, ID_SEED).expect("identify");
        let fit = fit_modal(
            &h,
            FS_HZ,
            &FitOptions {
                n_modes,
                ..Default::default()
            },
        )
        .expect("fit");
        ModelCase {
            setup: research_setup_signed(h, &pol),
            modal: fit.model,
        }
    } else {
        ModelCase {
            setup: research_setup_signed(plant.frf(S_ID, FS_HZ, RECORD_LEN), &pol),
            modal: plant.modal_truth(S_ID),
        }
    }
}

fn run(
    s: &Spec,
    mc: &ModelCase,
    respond: &mut Respond<'_>,
    ladder: &[f64],
    noise: f64,
) -> Vec<ResearchRow> {
    let mut law = build(s, &mc.setup, &mc.modal);
    run_research_ladder(law.as_mut(), &mc.setup, respond, ladder, noise, SEED, None).expect("run")
}

fn line(name: &str, rows: &[ResearchRow], first_full: usize) -> String {
    let at = |k: usize| rows.get(k - 1).map(|r| r.worst_db).unwrap_or(f64::NAN);
    let grow = rows
        .iter()
        .map(|r| r.drive_rms / rows[0].drive_rms)
        .fold(0.0f64, f64::max);
    let c = rows
        .iter()
        .map(|r| r.value("gain_extent_db"))
        .filter(|v| v.is_finite())
        .fold(f64::NAN, f64::max);
    let gate = rows
        .iter()
        .filter(|r| r.value("model_gate_fired") == 1.0)
        .count();
    let res = |k: usize| {
        rows.get(k - 1)
            .map(|r| r.value("model_prediction_residual"))
            .unwrap_or(f64::NAN)
    };
    let f = first_full;
    format!(
        "  {:38} {:+6.1} {:5.1} ({:+5.1}) {:5.1} {:5.1} {:5.1} {:5.2} {:6.1} {:6.3} {:6.3} {:3}",
        name,
        rows[0].over_db,
        at(f),
        rows[f - 1].over_db,
        at(f + 1),
        at(f + 3),
        at(rows.len().min(f + 9)),
        grow,
        c,
        res(f),
        res(f + 3),
        gate
    )
}

fn header() -> &'static str {
    "  law                                      p1+    p5 (over)    p6    p8   p14  grow      C   res5   res8 gate"
}

fn four_by_fours() -> Vec<(&'static str, LevelPlant)> {
    vec![
        (
            "4x4 8% drop, damping fixed",
            LevelPlant::four_by_four_damped(0.08, 0.0, 0.02),
        ),
        (
            "4x4 5% drop, damping x2",
            LevelPlant::four_by_four_damped(0.05, 1.0, 0.02),
        ),
        (
            "4x4 5% drop, damping fixed",
            LevelPlant::four_by_four_damped(0.05, 0.0, 0.02),
        ),
    ]
}

fn part_main(only_fitted: Option<bool>) {
    for (pname, plant) in four_by_fours() {
        for fitted in [false, true] {
            if only_fitted.is_some_and(|o| o != fitted) {
                continue;
            }
            for mixed in [false, true] {
                let mc = model_case(&plant, mixed, fitted, 4);
                for noise in [0.0, 0.30] {
                    println!(
                        "\n== {pname}; model {}; targets {}; noise {:.0}% of record rms",
                        if fitted { "FITTED from H1" } else { "true" },
                        if mixed { "mixed" } else { "in phase" },
                        100.0 * noise
                    );
                    println!("{}", header());
                    for s in main_laws() {
                        let mut resp = |d: &[Vec<f64>], sc: f64| plant.respond(d, sc, FS_HZ);
                        let rows = run(&s, &mc, &mut resp, &LADDER_DB, noise);
                        println!("{}", line(s.name, &rows, 5));
                    }
                }
            }
        }
    }
}

/// Per mode, the share of the seed's in-band response energy it carries, and
/// the seed's measured and predicted over-test.
fn part_excite() {
    let mut plants = four_by_fours();
    plants.push((
        "4x4 close modes, 8%",
        LevelPlant::four_by_four_close(0.08, 0.0),
    ));
    for (pname, plant) in plants {
        for mixed in [false, true] {
            for fitted in [false, true] {
                let n_modes = plant.modes.len();
                let mc = model_case(&plant, mixed, fitted, n_modes);
                let mut law = ShapeLaw::new(mc.setup.clone(), ShapeParams::default()).unwrap();
                let out = law.seed(LADDER_DB[0]).unwrap();
                let s = 10f64.powf(LADDER_DB[0] / 20.0);
                let d = rows_rfft(&out.drives, s).unwrap();
                let truth = plant.modal_truth(s);
                let band = fit_band();
                let df = FS_HZ / RECORD_LEN as f64;
                let mut shares = vec![0.0; truth.modes.len()];
                for (f, &on) in band.iter().enumerate() {
                    if !on {
                        continue;
                    }
                    let w = TAU * f as f64 * df;
                    for (m, mode) in truth.modes.iter().enumerate() {
                        let q = hertzforge_mimo_shock_law_math::modal::modal_q(
                            w,
                            TAU * mode.fn_hz,
                            mode.zeta,
                        );
                        for r in 0..truth.n_c {
                            let mut acc = C64::new(0.0, 0.0);
                            for (c, dc) in d.iter().enumerate() {
                                acc += mode.residue[r * truth.n_d + c] * dc[f];
                            }
                            shares[m] += (q * acc).norm_sqr();
                        }
                    }
                }
                let tot: f64 = shares.iter().sum();
                let y = plant.respond(&out.drives, s, FS_HZ).unwrap();
                let tgt = mc.setup.srs.as_ref().unwrap();
                let srs_m = srs_rows(&y, FS_HZ, &tgt.natural_hz, tgt.q, s);
                let pred_spec = apply(&mc.setup.plant.frf, &d);
                let pred = rows_irfft(&pred_spec, RECORD_LEN, 1.0).unwrap();
                let srs_p = srs_rows(&pred, FS_HZ, &tgt.natural_hz, tgt.q, 1.0);
                let over = |srs: &[Vec<f64>]| {
                    srs.iter()
                        .zip(&tgt.points)
                        .flat_map(|(m, t)| error_db(m, t))
                        .fold(f64::NEG_INFINITY, f64::max)
                };
                println!(
                    "{pname:28} {:8} {:6}: seed over measured {:+5.2} dB, predicted {:+5.2} dB; mode shares {}",
                    if mixed { "mixed" } else { "in phase" },
                    if fitted { "fitted" } else { "true" },
                    over(&srs_m),
                    over(&srs_p),
                    truth
                        .modes
                        .iter()
                        .zip(&shares)
                        .map(|(m, sh)| format!("{:.0} Hz {:.1}%", m.fn_hz, 100.0 * sh / tot))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }
}

fn part_stress() {
    let shape_base = spec("shape [baseline default]", Kind::Shape, Prov::None);
    let inv_secant = spec(
        "inverse + secant (ungated) [B2]",
        Kind::Inverse,
        Prov::Secant(0.0),
    );
    let shape_secant = spec(
        "shape + secant [fallback alone]",
        Kind::Shape,
        Prov::Secant(0.0),
    );
    let ex = spec("shape + track extrap [B1]", Kind::Shape, track(true, false));
    let ex_nogate = spec(
        "shape + track extrap, no gate",
        Kind::Shape,
        Prov::Track {
            extrapolate: true,
            gate: None,
            secant: false,
            sparse: Sparse::None,
            refit: false,
        },
    );
    let ex_refit = spec(
        "shape + track extrap + residue refit",
        Kind::Shape,
        Prov::Track {
            extrapolate: true,
            gate: Some(1.2),
            secant: false,
            sparse: Sparse::None,
            refit: true,
        },
    );
    let ex_sec = spec(
        "shape + track extrap + secant",
        Kind::Shape,
        track(true, true),
    );
    let inv_ex_sec = Spec {
        name: "inverse + track extrap + secant, pred. step",
        predicted_step: true,
        ..spec("", Kind::Inverse, track(true, true))
    };
    let laws = [
        shape_base,
        shape_secant,
        inv_secant,
        ex,
        ex_nogate,
        ex_refit,
        ex_sec,
        inv_ex_sec,
    ];
    let ladder: Vec<f64> = [-12.0, -9.0, -6.0, -3.0]
        .into_iter()
        .chain([0.0; 16])
        .collect();

    let base = LevelPlant::four_by_four_damped(0.08, 0.0, 0.02);
    let mut quad = base.clone();
    quad.drop_law = DropLaw::Quadratic;
    let shapes = LevelPlant::four_by_four_shapes_move(0.08, 0.0, 0.10, 5);
    let close = LevelPlant::four_by_four_close(0.08, 0.0);
    for mixed in [false, true] {
        let pol = polarity(4, mixed);
        let tname = if mixed { "mixed" } else { "in phase" };
        // Residues 15% wrong: the modal model from a plant with shape errors.
        let wrong = base.with_shape_errors(0.15, 3);
        let mc = ModelCase {
            setup: research_setup_signed(base.frf(S_ID, FS_HZ, RECORD_LEN), &pol),
            modal: wrong.modal_truth(S_ID),
        };
        let cases: Vec<(String, &LevelPlant, ModelCase, f64)> = vec![
            (format!("residues 15% wrong; {tname}"), &base, mc, 0.0),
            (
                format!("shapes move 10% at full level, true model at -20 dB; {tname}"),
                &shapes,
                ModelCase {
                    setup: research_setup_signed(shapes.frf(S_ID, FS_HZ, RECORD_LEN), &pol),
                    modal: shapes.modal_truth(S_ID),
                },
                0.0,
            ),
            (
                format!("shapes move 10%, fitted model; {tname}"),
                &shapes,
                model_case(&shapes, mixed, true, 4),
                0.0,
            ),
            (
                format!("drop quadratic in level, true model; {tname}"),
                &quad,
                ModelCase {
                    setup: research_setup_signed(quad.frf(S_ID, FS_HZ, RECORD_LEN), &pol),
                    modal: quad.modal_truth(S_ID),
                },
                0.0,
            ),
            (
                format!("close modes (1060 Hz drops 1%), fitted 5 modes; {tname}"),
                &close,
                model_case(&close, mixed, true, 5),
                0.0,
            ),
        ];
        for (name, plant, mc, noise) in cases {
            println!("\n== {name}");
            println!("{}", header());
            for s in &laws {
                let mut resp = |d: &[Vec<f64>], sc: f64| plant.respond(d, sc, FS_HZ);
                let rows = run(s, &mc, &mut resp, &ladder, noise);
                println!("{}  p20 {:.1}", line(s.name, &rows, 5), rows[19].worst_db);
            }
        }
    }
    // The intra-pulse plant: identified by H1 at -20 dB from its own time
    // simulation, fitted, then the ladder.
    let ip = IntraPulsePlant::four_by_four(0.08, 1.0).expect("plant");
    let mut rng = hertzforge_mimo_shock_law_math::reference_plant::Rng::new(ID_SEED);
    let len = RECORD_LEN * 16;
    let lin = ip.linear_frf(FS_HZ, RECORD_LEN);
    // Drive level for the identification: random drives whose response rms is
    // that of the reference seed at -20 dB on the linear plant.
    let dr = hertzforge_mimo_shock_law_math::research_plant::random_drives(4, len, 1.0, &mut rng);
    let y0 = ip.respond(&dr, FS_HZ);
    let r0 = (y0.iter().flatten().map(|v| v * v).sum::<f64>() / (4 * len) as f64).sqrt();
    let refr = hertzforge_mimo_shock_law_math::reference_plant::reference_shock(FS_HZ, RECORD_LEN);
    let ref_rms = (refr.iter().map(|v| v * v).sum::<f64>() / RECORD_LEN as f64).sqrt();
    let k = 0.1 * ref_rms / r0;
    let dr: Vec<Vec<f64>> = dr
        .iter()
        .map(|d| d.iter().map(|v| v * k).collect())
        .collect();
    let mut y = ip.respond(&dr, FS_HZ);
    hertzforge_mimo_shock_law_math::research_plant::add_relative_noise(&mut y, 0.05, &mut rng);
    let h = hertzforge_mimo_shock_law_math::research_plant::h1_from_records(&dr, &y, RECORD_LEN)
        .expect("h1");
    let fit = fit_modal(&h, FS_HZ, &FitOptions::default()).expect("fit");
    println!(
        "\n== intra-pulse plant (delta 0.08, kappa 1): fit residual vs its H1 {:.4}; H1 vs linear {:.4}",
        fit.residual_rel,
        hertzforge_mimo_shock_law_math::modal::relative_residual(
            &fit.model,
            &lin,
            &fit.band,
            FS_HZ
        )
    );
    for mixed in [false, true] {
        println!("-- targets {}", if mixed { "mixed" } else { "in phase" });
        let mc = ModelCase {
            setup: research_setup_signed(h.clone(), &polarity(4, mixed)),
            modal: fit.model.clone(),
        };
        println!("{}", header());
        let inv_base = spec(
            "inverse+watchdog [baseline default]",
            Kind::Inverse,
            Prov::None,
        );
        for s in [
            &shape_base,
            &inv_base,
            &shape_secant,
            &inv_secant,
            &ex,
            &ex_sec,
            &inv_ex_sec,
        ] {
            let mut resp = |d: &[Vec<f64>], _sc: f64| Ok(ip.respond(d, FS_HZ));
            let rows = run(s, &mc, &mut resp, &LADDER_DB, 0.0);
            println!("{}", line(s.name, &rows, 5));
        }
    }
    // The rectangular 2 x 3.
    let p23 = LevelPlant::from_modal(&ModalPlant::two_by_three(0.08, 0.0));
    println!("\n== 2 x 3, 8% drop, fitted model (2 modes); in phase");
    println!("{}", header());
    let mc = model_case(&p23, false, true, 2);
    for s in &laws {
        let mut resp = |d: &[Vec<f64>], sc: f64| p23.respond(d, sc, FS_HZ);
        let rows = run(s, &mc, &mut resp, &ladder, 0.0);
        println!("{}  p20 {:.1}", line(s.name, &rows, 5), rows[19].worst_db);
    }
}

fn part_sparse() {
    let laws: Vec<(&str, Sparse)> = vec![
        ("track extrap, plain GN", Sparse::None),
        (
            "track extrap, lasso 0.05",
            Sparse::Lasso { lambda_rel: 0.05 },
        ),
        ("track extrap, lasso 0.2", Sparse::Lasso { lambda_rel: 0.2 }),
        (
            "track extrap, group lasso 0.05",
            Sparse::GroupLasso { lambda_rel: 0.05 },
        ),
        (
            "track extrap, group lasso 0.2",
            Sparse::GroupLasso { lambda_rel: 0.2 },
        ),
    ];
    let plant = LevelPlant::four_by_four_damped(0.08, 0.0, 0.02);
    let close = LevelPlant::four_by_four_close(0.08, 0.0);
    let cases: Vec<(&str, &LevelPlant, usize, f64)> = vec![
        ("4x4 8%, fitted, noise 30%", &plant, 4, 0.30),
        (
            "4x4 8%, fitted, order one high (5), noise 30%",
            &plant,
            5,
            0.30,
        ),
        (
            "4x4 8%, fitted, order two high (6), noise 30%",
            &plant,
            6,
            0.30,
        ),
        ("4x4 8%, fitted, order one low (3), clean", &plant, 3, 0.0),
        (
            "close modes (one barely moves), fitted 5, noise 30%",
            &close,
            5,
            0.30,
        ),
    ];
    for (name, p, order, noise) in cases {
        let mc = model_case(p, true, true, order);
        println!("\n== {name}; targets mixed");
        println!("{}", header());
        for (lname, sp) in &laws {
            let s = Spec {
                name: lname,
                kind: Kind::Shape,
                prov: Prov::Track {
                    extrapolate: true,
                    gate: Some(1.2),
                    secant: false,
                    sparse: *sp,
                    refit: false,
                },
                safety: false,
                robust: false,
                predicted_step: false,
            };
            let mut resp = |d: &[Vec<f64>], sc: f64| p.respond(d, sc, FS_HZ);
            let rows = run(&s, &mc, &mut resp, &LADDER_DB, noise);
            println!("{}", line(lname, &rows, 5));
        }
    }
}

fn part_safety() {
    let plant = LevelPlant::four_by_four_damped(0.08, 0.0, 0.02);
    let ladder: Vec<f64> = [-12.0, -9.0, -6.0, -3.0]
        .into_iter()
        .chain([0.0; 96])
        .collect();
    for mixed in [false, true] {
        let mc = model_case(&plant, mixed, false, 4);
        for (name, safety) in [
            ("shape, no clamp [baseline]", false),
            ("shape + B3 safety", true),
        ] {
            let mut law = ShapeLaw::with_options(
                mc.setup.clone(),
                ShapeParams::default(),
                ShapeExtras {
                    safety: if safety {
                        ShapeSafety::recommended()
                    } else {
                        ShapeSafety::off()
                    },
                    ..Default::default()
                },
            )
            .unwrap();
            let mut resp = |d: &[Vec<f64>], sc: f64| plant.respond(d, sc, FS_HZ);
            let rows =
                run_research_ladder(&mut law, &mc.setup, &mut resp, &ladder, 0.0, SEED, None)
                    .unwrap();
            let first_stall = rows.iter().position(|r| r.value("stalled_points") > 0.0);
            let cmax = rows
                .iter()
                .map(|r| r.value("gain_extent_db"))
                .fold(0.0f64, f64::max);
            println!(
                "{:8} {name:28}: C max {cmax:6.1} dB (p14 {:.1}, p100 {:.1}); worst p14 {:.1} p40 {:.1} p100 {:.1}; first stall flag at p{}",
                if mixed { "mixed" } else { "in phase" },
                rows[13].value("gain_extent_db"),
                rows[99].value("gain_extent_db"),
                rows[13].worst_db,
                rows[39].worst_db,
                rows[99].worst_db,
                first_stall.map(|k| k + 1).unwrap_or(0)
            );
        }
        // Clipped drive: drive 1 at 60% of its seed peak.
        for (name, safety) in [
            ("shape, clipped [baseline]", false),
            ("shape + B3, clipped", true),
        ] {
            let mut law: Box<dyn LawSteps> = Box::new(
                ShapeLaw::with_options(
                    mc.setup.clone(),
                    ShapeParams::default(),
                    ShapeExtras {
                        safety: if safety {
                            ShapeSafety::recommended()
                        } else {
                            ShapeSafety::off()
                        },
                        ..Default::default()
                    },
                )
                .unwrap(),
            );
            let mut resp = |d: &[Vec<f64>], sc: f64| plant.respond(d, sc, FS_HZ);
            let rows = run_research_ladder(
                law.as_mut(),
                &mc.setup,
                &mut resp,
                &ladder[..20],
                0.0,
                SEED,
                Some(Ceiling {
                    drive: 0,
                    fraction: 0.6,
                }),
            )
            .unwrap();
            println!(
                "{:8} {name:28}: C at p5 {:.1}, p10 {:.1}, p20 {:.1} dB; worst p20 {:.1}",
                if mixed { "mixed" } else { "in phase" },
                rows[4].value("gain_extent_db"),
                rows[9].value("gain_extent_db"),
                rows[19].value("gain_extent_db"),
                rows[19].worst_db
            );
        }
    }
}

fn part_secant() {
    let plant = LevelPlant::four_by_four_damped(0.08, 0.0, 0.02);
    for mixed in [false, true] {
        let mc = model_case(&plant, mixed, false, 4);
        for gate in [0.0, 0.1, 0.2, 0.4] {
            let s = spec("inverse + secant", Kind::Inverse, Prov::Secant(gate));
            let mut resp = |d: &[Vec<f64>], sc: f64| plant.respond(d, sc, FS_HZ);
            let rows = run(&s, &mc, &mut resp, &LADDER_DB, 0.30);
            let w: Vec<String> = rows.iter().map(|r| format!("{:.1}", r.worst_db)).collect();
            println!(
                "{:8} gate {gate:.1}: worst by pulse {}; p6 - p5 {:+.1} dB",
                if mixed { "mixed" } else { "in phase" },
                w.join(" "),
                rows[5].worst_db - rows[4].worst_db
            );
        }
    }
}

fn part_cost() {
    for (name, plant, nm) in [
        (
            "4 x 4",
            LevelPlant::four_by_four_damped(0.08, 0.0, 0.02),
            4usize,
        ),
        (
            "16 x 8",
            LevelPlant::sixteen_by_eight(0.08, 0.0, 11),
            8usize,
        ),
    ] {
        let t0 = Instant::now();
        let mc = model_case(&plant, true, true, nm);
        let t_fit = t0.elapsed().as_secs_f64();
        let band = fit_band();
        let h: MatrixStack = mc.setup.plant.frf.clone();
        let mut p = PoleTrackModel::new(
            mc.modal.clone(),
            h.clone(),
            band.clone(),
            FS_HZ,
            TrackOptions::default(),
            Box::new(FrozenModel::new(h, band)),
        )
        .unwrap();
        let mut law = ShapeLaw::new(mc.setup.clone(), ShapeParams::default()).unwrap();
        let out = law.seed(-12.0).unwrap();
        let s = 10f64.powf(-12.0 / 20.0);
        let y = plant.respond(&out.drives, s, FS_HZ).unwrap();
        let d = rows_rfft(&out.drives, s).unwrap();
        let yy = rows_rfft(&y, s).unwrap();
        let t1 = Instant::now();
        let reps = 5;
        for _ in 0..reps {
            p.observe(&hertzforge_mimo_shock_law_math::provider::Observation {
                level_db: -12.0,
                drives: &d,
                responses: &yy,
            })
            .unwrap();
            p.model_for(-9.0).unwrap();
        }
        println!(
            "{name}: identification and fit {t_fit:.2} s; tracking update and next model {:.3} s a pulse",
            t1.elapsed().as_secs_f64() / reps as f64
        );
    }
}

/// The first pulse: the seed against a starter pulse 6 dB below the ladder
/// that only starts the tracking.
fn part_seed() {
    let b1 = spec("shape + track extrap [B1]", Kind::Shape, track(true, false));
    let b1id = spec(
        "shape + track from the identification",
        Kind::Shape,
        Prov::TrackFromId,
    );
    let mut plants = four_by_fours();
    plants.push((
        "4x4 close modes, 8%",
        LevelPlant::four_by_four_close(0.08, 0.0),
    ));
    for (pname, plant) in plants {
        for mixed in [false, true] {
            let nm = plant.modes.len();
            let mc = model_case(&plant, mixed, true, nm);
            let mut resp = |d: &[Vec<f64>], sc: f64| plant.respond(d, sc, FS_HZ);
            let plain = run(&b1, &mc, &mut resp, &LADDER_DB, 0.0);
            let starter: Vec<f64> = std::iter::once(-18.0).chain(LADDER_DB).collect();
            let st = run(&b1, &mc, &mut resp, &starter, 0.0);
            let id = run(&b1id, &mc, &mut resp, &LADDER_DB, 0.0);
            let idst = run(&b1id, &mc, &mut resp, &starter, 0.0);
            println!(
                "{pname:28} {:8}: identification as a level point (with secant): -12 {:+5.2}, -9 {:+5.2}, p5 {:+5.2}, p8 {:.2} | with the starter: starter {:+5.2}, -12 {:+5.2}, -9 {:+5.2}, first 0 dB {:+5.2}",
                if mixed { "mixed" } else { "in phase" },
                id[0].over_db, id[1].over_db, id[4].over_db, id[7].worst_db,
                idst[0].over_db, idst[1].over_db, idst[2].over_db, idst[5].over_db
            );
            println!(
                "{pname:28} {:8}: no starter: -12 dB over {:+5.2}, -9 {:+5.2}, p5 {:+5.2}, p8 {:.2} | starter at -18: starter over {:+5.2}, -12 dB {:+5.2}, -9 {:+5.2}, first 0 dB {:+5.2}, its p8 {:.2}",
                if mixed { "mixed" } else { "in phase" },
                plain[0].over_db, plain[1].over_db, plain[4].over_db, plain[7].worst_db,
                st[0].over_db, st[1].over_db, st[2].over_db, st[5].over_db, st[8].worst_db
            );
        }
    }
}

/// Why `inverse` with its watchdog diverges on the realistic identification.
fn part_h1() {
    let plant = LevelPlant::four_by_four_damped(0.08, 0.0, 0.02);
    for (label, fitted) in [("exact identification", false), ("realistic H1", true)] {
        let mc = model_case(&plant, false, fitted, 4);
        let mut banded = mc.setup.clone();
        banded.band_lo_hz = 80.0;
        banded.band_hi_hz = 2400.0;
        for (name, setup, watchdog) in [
            ("inverse, watchdog, every line driven", &mc.setup, true),
            ("inverse, no watchdog, every line driven", &mc.setup, false),
            ("inverse, watchdog, band 80 to 2400 Hz", &banded, true),
            ("inverse, no watchdog, band 80 to 2400 Hz", &banded, false),
        ] {
            let mut law = InverseLaw::new(
                setup.clone(),
                InverseParams {
                    watchdog,
                    ..Default::default()
                },
            )
            .unwrap();
            let mut resp = |d: &[Vec<f64>], sc: f64| plant.respond(d, sc, FS_HZ);
            let rows = run_research_ladder(&mut law, setup, &mut resp, &LADDER_DB, 0.0, SEED, None)
                .unwrap();
            println!("{label:22} {}", line(name, &rows, 5));
        }
    }
}

fn main() {
    let part = std::env::args().nth(1).unwrap_or_else(|| "main".into());
    match part.as_str() {
        "main" => part_main(None),
        "main-true" => part_main(Some(false)),
        "main-fitted" => part_main(Some(true)),
        "excite" => part_excite(),
        "stress" => part_stress(),
        "sparse" => part_sparse(),
        "safety" => part_safety(),
        "secant" => part_secant(),
        "cost" => part_cost(),
        "seed" => part_seed(),
        "h1" => part_h1(),
        other => eprintln!("unknown part {other}"),
    }
}
