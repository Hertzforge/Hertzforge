//! The MIMO Shock follower study: the frozen model, the same-level secant,
//! pole tracking, the warp follower and the choice between them, with `shape`
//! and `inverse`, on the research plants, with the true and the realistic
//! identification, clean and noisy. Fixed seeds, no file output; it prints
//! tables.
//!
//! ```text
//! cargo run --release -p hertzforge-mimo-shock-law-math --features reference-plant \
//!     --example mimo_shock_follower_study -- <part> [plant]
//! ```
//!
//! Parts: `crosscheck` (one fit from one pulse, printed for comparison with
//! another implementation), `compare [plant]` (the one-page comparison; plant
//! one of `8`, `5x2`, `5`, `close`, `dense`, `narrow`, `intra`, `shapes`,
//! `2x3`, or all), `gate`, `combos`, `cost`, `seeds` (the warp over eight
//! noise seeds), `diag [plant]` (the choice held on each follower).
//!
//! Columns: `p1+` the first pulse's signed over-test; `p5+` the first
//! full-level pulse's signed over-test with its worst error in brackets; `p8`
//! and `p14` worst errors; `grow` largest drive rms over the first pulse's;
//! `ms` wall time a pulse (law, follower and plant); `pick` the follower the
//! choice gave the law after each pulse, for the next (`W` warp, `T` pole
//! tracking, `m` mixed by region; the fourth letter is the model of p5).

use hertzforge_mimo_shock_law_math::choice::{ChoiceLog, ChoiceModel, ChoiceOptions};
use hertzforge_mimo_shock_law_math::inverse::{InverseExtras, InverseLaw, InverseParams};
use hertzforge_mimo_shock_law_math::modal::{fit_modal, FitOptions, ModalModel};
use hertzforge_mimo_shock_law_math::model::regularized_inverse;
use hertzforge_mimo_shock_law_math::provider::{
    ModelProvider, MultiSecantModel, Observation, PoleTrackModel, SecantOptions, TrackOptions,
};
use hertzforge_mimo_shock_law_math::reference_plant::{
    reference_shock, ModalPlant, Rng, FS_HZ, LADDER_DB, RECORD_LEN,
};
use hertzforge_mimo_shock_law_math::research_plant::{
    add_relative_noise, h1_from_records, identify_h1_model, random_drives, research_setup_signed,
    run_research_ladder, IntraPulsePlant, LevelPlant, ResearchRow, Respond, SpreadPlant,
};
use hertzforge_mimo_shock_law_math::shape::{ShapeExtras, ShapeLaw, ShapeParams, ShapeSafety};
use hertzforge_mimo_shock_law_math::spectrum::{apply, rfft};
use hertzforge_mimo_shock_law_math::warp::{WarpModel, WarpOptions, WarpSecant};
use hertzforge_mimo_shock_law_math::{level_scale, LawSteps, MatrixStack, Setup, C64};
use std::time::Instant;

const S_ID: f64 = 0.1;
const SEED: u64 = 20_261_111;
const ID_SEED: u64 = 7;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Shape,
    Inverse,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Fol {
    Frozen,
    Secant,
    Track,
    Warp,
    WarpPlainSecant,
    WarpGated,
    WarpNoSecant,
    Choice,
    ChoiceBands,
    ChoiceTrackFirst,
}

#[derive(Clone, Copy)]
struct Row {
    name: &'static str,
    kind: Kind,
    fol: Fol,
    predicted_step: bool,
}

const fn row(name: &'static str, kind: Kind, fol: Fol) -> Row {
    Row {
        name,
        kind,
        fol,
        predicted_step: false,
    }
}

fn compare_rows() -> Vec<Row> {
    vec![
        row("shape frozen", Kind::Shape, Fol::Frozen),
        row("shape secant", Kind::Shape, Fol::Secant),
        row("shape track+secant", Kind::Shape, Fol::Track),
        row("shape warp+secant", Kind::Shape, Fol::Warp),
        row("shape choice", Kind::Shape, Fol::Choice),
        row("shape choice by band", Kind::Shape, Fol::ChoiceBands),
        row(
            "shape choice, tracking first",
            Kind::Shape,
            Fol::ChoiceTrackFirst,
        ),
        row("inverse frozen+watchdog", Kind::Inverse, Fol::Frozen),
        row("inverse secant", Kind::Inverse, Fol::Secant),
        Row {
            predicted_step: true,
            ..row("inverse track+secant, step", Kind::Inverse, Fol::Track)
        },
        row("inverse warp+secant", Kind::Inverse, Fol::Warp),
        Row {
            predicted_step: true,
            ..row("inverse warp+secant, step", Kind::Inverse, Fol::Warp)
        },
        Row {
            predicted_step: true,
            ..row("inverse choice, step", Kind::Inverse, Fol::Choice)
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

type PlantFn = dyn Fn(&[Vec<f64>], f64) -> Result<Vec<Vec<f64>>, String>;
type Built = (Option<Box<dyn ModelProvider>>, Option<ChoiceLog>);

/// A plant of the study: how it responds, its truth at the identification
/// level, and its realistic identification.
struct Case {
    name: String,
    setup_true: Setup,
    setup_h1: Setup,
    modal_true: ModalModel,
    modal_h1: Option<ModalModel>,
    respond: Box<PlantFn>,
    n_c: usize,
}

fn polarity(n_c: usize, mixed: bool) -> Vec<f64> {
    (0..n_c)
        .map(|c| if mixed && c % 2 == 1 { -1.0 } else { 1.0 })
        .collect()
}

fn fitted(h: &MatrixStack, n_modes: usize) -> Option<ModalModel> {
    fit_modal(
        h,
        FS_HZ,
        &FitOptions {
            n_modes,
            ..Default::default()
        },
    )
    .ok()
    .map(|f| f.model)
}

fn level_case(name: &str, p: LevelPlant) -> Case {
    let truth = p.modal_truth(S_ID);
    let h1 = identify_h1_model(&truth, 16, 0.05, ID_SEED).expect("identify");
    let n = p.modes.len();
    let n_c = p.n_c;
    Case {
        name: name.into(),
        setup_true: research_setup_signed(p.frf(S_ID, FS_HZ, RECORD_LEN), &vec![1.0; n_c]),
        modal_h1: fitted(&h1, n),
        setup_h1: research_setup_signed(h1, &vec![1.0; n_c]),
        modal_true: truth,
        respond: Box::new(move |d, s| p.respond(d, s, FS_HZ)),
        n_c,
    }
}

fn spread_case(name: &str, p: SpreadPlant) -> Case {
    let truth = p.modal_truth(S_ID);
    let h1 = identify_h1_model(&truth, 16, 0.05, ID_SEED).expect("identify");
    let n = p.modes.len();
    let n_c = p.n_c;
    Case {
        name: name.into(),
        setup_true: research_setup_signed(p.frf(S_ID, FS_HZ, RECORD_LEN), &vec![1.0; n_c]),
        modal_h1: fitted(&h1, n),
        setup_h1: research_setup_signed(h1, &vec![1.0; n_c]),
        modal_true: truth,
        respond: Box::new(move |d, s| p.respond(d, s, FS_HZ)),
        n_c,
    }
}

/// The intra-pulse plant, identified by H1 from its own simulation at the
/// -20 dB response level (Package R's construction); its "true" case is the
/// linear plant.
fn intra_case() -> Case {
    let ip = IntraPulsePlant::four_by_four(0.08, 1.0).expect("plant");
    let mut rng = Rng::new(ID_SEED);
    let len = RECORD_LEN * 16;
    let dr = random_drives(4, len, 1.0, &mut rng);
    let y0 = ip.respond(&dr, FS_HZ);
    let r0 = (y0.iter().flatten().map(|v| v * v).sum::<f64>() / (4 * len) as f64).sqrt();
    let refr = reference_shock(FS_HZ, RECORD_LEN);
    let ref_rms = (refr.iter().map(|v| v * v).sum::<f64>() / RECORD_LEN as f64).sqrt();
    let k = 0.1 * ref_rms / r0;
    let dr: Vec<Vec<f64>> = dr
        .iter()
        .map(|d| d.iter().map(|v| v * k).collect())
        .collect();
    let mut y = ip.respond(&dr, FS_HZ);
    add_relative_noise(&mut y, 0.05, &mut rng);
    let h = h1_from_records(&dr, &y, RECORD_LEN).expect("h1");
    let lin = LevelPlant {
        n_c: 4,
        n_d: 4,
        quasi_static: ip.quasi_static.clone(),
        modes: (0..ip.wn0.len())
            .map(
                |m| hertzforge_mimo_shock_law_math::research_plant::LevelMode {
                    fn0_hz: ip.wn0[m] / std::f64::consts::TAU,
                    zeta0: ip.z0,
                    drop_scale: 1.0,
                    phi: ip.phi[m].clone(),
                    dphi: Vec::new(),
                    psi: ip.psi[m].clone(),
                    rotation: 0.0,
                },
            )
            .collect(),
        delta: 0.0,
        kappa: 0.0,
        delays_s: vec![0.0; 4],
        drop_law: hertzforge_mimo_shock_law_math::research_plant::DropLaw::Linear,
    };
    Case {
        name: "intra-pulse 8%, damping x2 at peak".into(),
        setup_true: research_setup_signed(lin.frf(0.0, FS_HZ, RECORD_LEN), &[1.0; 4]),
        modal_h1: fit_modal(&h, FS_HZ, &FitOptions::default())
            .ok()
            .map(|f| f.model),
        setup_h1: research_setup_signed(h, &[1.0; 4]),
        modal_true: lin.modal_truth(0.0),
        respond: Box::new(move |d, _s| Ok(ip.respond(d, FS_HZ))),
        n_c: 4,
    }
}

fn case(key: &str) -> Case {
    match key {
        "8" => level_case(
            "4x4 8% drop, damping fixed",
            LevelPlant::four_by_four_damped(0.08, 0.0, 0.02),
        ),
        "5x2" => level_case(
            "4x4 5% drop, damping x2",
            LevelPlant::four_by_four_damped(0.05, 1.0, 0.02),
        ),
        "5" => level_case(
            "4x4 5% drop, damping fixed",
            LevelPlant::four_by_four_damped(0.05, 0.0, 0.02),
        ),
        "close" => spread_case(
            "close modes 1000 Hz (8%) and 1060 Hz (1%)",
            SpreadPlant::four_by_four_close_apart(),
        ),
        "dense" => spread_case(
            "dense modes, 60 above 850 Hz, incoherent",
            SpreadPlant::dense(60, false, 31),
        ),
        "narrow" => level_case(
            "narrow modes (zeta 0.002), 8% drop",
            LevelPlant::four_by_four_damped(0.08, 0.0, 0.002),
        ),
        "intra" => intra_case(),
        "shapes" => level_case(
            "shapes move 10% at full level, 8% drop",
            LevelPlant::four_by_four_shapes_move(0.08, 0.0, 0.10, 5),
        ),
        "2x3" => level_case(
            "2x3 8% drop",
            LevelPlant::from_modal(&ModalPlant::two_by_three(0.08, 0.0)),
        ),
        other => panic!("unknown plant {other}"),
    }
}

const ALL: [&str; 9] = [
    "8", "5x2", "5", "close", "dense", "narrow", "intra", "shapes", "2x3",
];

fn warp_opts(secant: Option<WarpSecant>, gate: Option<f64>) -> WarpOptions {
    WarpOptions {
        identification_scale: Some(S_ID),
        secant,
        gate,
        ..Default::default()
    }
}

fn track(modal: &ModalModel, h: &MatrixStack) -> Result<Box<dyn ModelProvider>, String> {
    let band = fit_band();
    Ok(Box::new(PoleTrackModel::new(
        modal.clone(),
        h.clone(),
        band.clone(),
        FS_HZ,
        TrackOptions {
            secant: Some(SecantOptions::default()),
            ..Default::default()
        },
        Box::new(MultiSecantModel::new(
            h.clone(),
            band,
            SecantOptions::default(),
        )),
    )?))
}

/// A follower for a run; the choice's log when it is a choice.
fn follower(
    fol: Fol,
    setup: &Setup,
    modal: Option<&ModalModel>,
    warp: &WarpModel,
) -> Result<Built, String> {
    let h = setup.plant.frf.clone();
    let band = fit_band();
    let w = |secant, gate| -> Box<dyn ModelProvider> {
        let mut m = warp.clone();
        m.set_refinement(secant, gate);
        Box::new(m)
    };
    Ok(match fol {
        Fol::Frozen => (None, None),
        Fol::Secant => (
            Some(Box::new(MultiSecantModel::new(
                h,
                band,
                SecantOptions::default(),
            ))),
            None,
        ),
        Fol::Track => (Some(track(modal.ok_or("no modal fit")?, &h)?), None),
        Fol::Warp => (Some(w(Some(WarpSecant::NoiseRidge), None)), None),
        Fol::WarpPlainSecant => (
            Some(w(Some(WarpSecant::Plain(SecantOptions::default())), None)),
            None,
        ),
        Fol::WarpGated => (Some(w(Some(WarpSecant::NoiseRidge), Some(2.0))), None),
        Fol::WarpNoSecant => (Some(w(None, None)), None),
        Fol::Choice | Fol::ChoiceBands | Fol::ChoiceTrackFirst => {
            let wp = w(Some(WarpSecant::NoiseRidge), None);
            let tp = track(modal.ok_or("no modal fit")?, &h)?;
            let edges = if fol == Fol::ChoiceBands {
                vec![160.0, 320.0, 640.0, 1280.0]
            } else {
                Vec::new()
            };
            let c = ChoiceModel::new(
                wp,
                tp,
                band,
                FS_HZ,
                ChoiceOptions {
                    edges_hz: edges,
                    start: usize::from(fol == Fol::ChoiceTrackFirst),
                    ..Default::default()
                },
            )?;
            let log = c.log();
            (Some(Box::new(c)), Some(log))
        }
    })
}

fn build(
    r: &Row,
    setup: &Setup,
    modal: Option<&ModalModel>,
    warp: &WarpModel,
) -> Result<(Box<dyn LawSteps>, Option<ChoiceLog>), String> {
    let (provider, log) = follower(r.fol, setup, modal, warp)?;
    let law: Box<dyn LawSteps> = match r.kind {
        Kind::Shape => {
            if provider.is_none() {
                Box::new(ShapeLaw::new(setup.clone(), ShapeParams::default())?)
            } else {
                Box::new(ShapeLaw::with_options(
                    setup.clone(),
                    ShapeParams::default(),
                    ShapeExtras {
                        provider,
                        safety: ShapeSafety::off(),
                        robust_first: None,
                    },
                )?)
            }
        }
        Kind::Inverse => {
            let params = InverseParams {
                watchdog: provider.is_none(),
                ..Default::default()
            };
            if provider.is_none() {
                Box::new(InverseLaw::new(setup.clone(), params)?)
            } else {
                Box::new(InverseLaw::with_options(
                    setup.clone(),
                    params,
                    InverseExtras {
                        provider,
                        robust_first: None,
                        predicted_step: r.predicted_step,
                    },
                )?)
            }
        }
    };
    Ok((law, log))
}

fn picks(log: &Option<ChoiceLog>) -> String {
    let Some(l) = log else {
        return String::new();
    };
    let l = l.lock().expect("log");
    // Letter k is the choice made after pulse k, for pulse k + 1.
    let mut s = String::new();
    for rec in l.iter().take(14) {
        let ones = rec.chosen.iter().filter(|&&c| c == 1).count();
        s.push(if ones == 0 {
            'W'
        } else if ones == rec.chosen.len() {
            'T'
        } else {
            'm'
        });
    }
    if let Some(rec) = l.get(3) {
        let by: String = rec
            .chosen
            .iter()
            .map(|&c| if c == 0 { 'W' } else { 'T' })
            .collect();
        if rec.chosen.len() > 1 {
            s.push_str(&format!(" p5 by band {by}"));
        }
    }
    s
}

struct Outcome {
    p1: f64,
    p5: f64,
    p5w: f64,
    p8: f64,
    p14: f64,
    grow: f64,
    ms: f64,
}

fn outcome(rows: &[ResearchRow], secs: f64) -> Outcome {
    Outcome {
        p1: rows[0].over_db,
        p5: rows[4].over_db,
        p5w: rows[4].worst_db,
        p8: rows[7].worst_db,
        p14: rows[13].worst_db,
        grow: rows
            .iter()
            .map(|r| r.drive_rms / rows[0].drive_rms)
            .fold(0.0f64, f64::max),
        ms: 1e3 * secs / rows.len() as f64,
    }
}

fn header() -> &'static str {
    "  law and follower                 p1+    p5+ (worst)    p8   p14   grow     ms  pick"
}

fn run_row(
    r: &Row,
    setup: &Setup,
    modal: Option<&ModalModel>,
    warp: &WarpModel,
    respond: &mut Respond<'_>,
    ladder: &[f64],
    noise: f64,
) -> Option<(Outcome, String)> {
    let (mut law, log) = match build(r, setup, modal, warp) {
        Ok(v) => v,
        Err(e) => {
            println!("  {:30} not run: {e}", r.name);
            return None;
        }
    };
    let t0 = Instant::now();
    let rows = match run_research_ladder(law.as_mut(), setup, respond, ladder, noise, SEED, None) {
        Ok(v) => v,
        Err(e) => {
            println!("  {:30} failed: {e}", r.name);
            return None;
        }
    };
    Some((outcome(&rows, t0.elapsed().as_secs_f64()), picks(&log)))
}

fn print_outcome(name: &str, o: &Outcome, pick: &str) {
    println!(
        "  {:30} {:+5.1} {:+6.1} ({:5.1}) {:5.1} {:5.1} {:6.2} {:6.0}  {}",
        name, o.p1, o.p5, o.p5w, o.p8, o.p14, o.grow, o.ms, pick
    );
}

fn base_warp(setup: &Setup) -> WarpModel {
    WarpModel::new(
        setup.plant.frf.clone(),
        fit_band(),
        FS_HZ,
        warp_opts(Some(WarpSecant::NoiseRidge), None),
    )
    .expect("warp")
}

fn with_polarity(setup: &Setup, mixed: bool) -> Setup {
    let mut s = research_setup_signed(setup.plant.frf.clone(), &polarity(setup.n_controls, mixed));
    s.plant = setup.plant.clone();
    s
}

fn part_compare(only: Option<&str>) {
    let keys: Vec<&str> = match only {
        Some(k) => vec![k],
        None => ALL.to_vec(),
    };
    for key in keys {
        let c = case(key);
        for h1 in [false, true] {
            let (setup0, modal) = if h1 {
                (&c.setup_h1, c.modal_h1.as_ref())
            } else {
                (&c.setup_true, Some(&c.modal_true))
            };
            let warp0 = base_warp(setup0);
            for mixed in [false, true] {
                let setup = with_polarity(setup0, mixed);
                for noise in [0.0, 0.30] {
                    println!(
                        "\n== {}; identification {}; targets {}; noise {:.0}%",
                        c.name,
                        if h1 { "H1" } else { "true" },
                        if mixed { "mixed" } else { "in phase" },
                        100.0 * noise
                    );
                    println!("{}", header());
                    for r in compare_rows() {
                        let mut resp = |d: &[Vec<f64>], s: f64| (c.respond)(d, s);
                        if let Some((o, pick)) =
                            run_row(&r, &setup, modal, &warp0, &mut resp, &LADDER_DB, noise)
                        {
                            print_outcome(r.name, &o, &pick);
                        }
                    }
                }
            }
        }
        let _ = c.n_c;
    }
}

/// One pulse at -12 dB on the 4 x 4 at 8% with the exact identification: the
/// drives the regularized inverse of the identification applied to the
/// reference on every control, the response exact. Prints the fitted knot
/// parameters for comparison with another implementation of the same fit.
fn part_crosscheck() {
    let p = LevelPlant::four_by_four_damped(0.08, 0.0, 0.02);
    let hid = p.frf(S_ID, FS_HZ, RECORD_LEN);
    let g = regularized_inverse(&hid, &vec![true; hid.lines()], 1e-3).expect("inverse");
    let r = rfft(&reference_shock(FS_HZ, RECORD_LEN)).expect("fft");
    let d = apply(&g, &vec![r; 4]);
    let s = level_scale(-12.0);
    let y = apply(&p.frf(s, FS_HZ, RECORD_LEN), &d);
    let mut w = WarpModel::new(
        hid.clone(),
        fit_band(),
        FS_HZ,
        warp_opts(Some(WarpSecant::NoiseRidge), None),
    )
    .expect("warp");
    println!(
        "delays (ms): {:?}",
        w.delays().iter().map(|v| v * 1e3).collect::<Vec<_>>()
    );
    w.observe(&Observation {
        level_db: -12.0,
        drives: &d,
        responses: &y,
    })
    .expect("observe");
    let x = w.parameters();
    let nk = x.len() / 2;
    println!("knots {nk}");
    println!(
        "1/a - 1: {}",
        x[..nk]
            .iter()
            .map(|v| format!("{v:.6e}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!(
        "sigma:   {}",
        x[nk..]
            .iter()
            .map(|v| format!("{v:.6e}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    w.model_for(-9.0).expect("model");
    let truth = p.frf(level_scale(-9.0), FS_HZ, RECORD_LEN);
    println!(
        "model for -9 dB against the plant there: {:.5} (identification {:.5})",
        hertzforge_mimo_shock_law_math::choice::model_distance(w.current(), &truth, &fit_band()),
        hertzforge_mimo_shock_law_math::choice::model_distance(&hid, &truth, &fit_band())
    );
}

/// The validity gate against the ungated warp.
fn part_gate() {
    let rows = [
        row("shape warp+secant", Kind::Shape, Fol::Warp),
        row("shape warp+secant, gated", Kind::Shape, Fol::WarpGated),
    ];
    for key in ["8", "5x2", "5", "close", "dense"] {
        let c = case(key);
        for h1 in [false, true] {
            let setup0 = if h1 { &c.setup_h1 } else { &c.setup_true };
            let warp0 = base_warp(setup0);
            let setup = with_polarity(setup0, true);
            for noise in [0.0, 0.30] {
                println!(
                    "\n== {}; identification {}; targets mixed; noise {:.0}%",
                    c.name,
                    if h1 { "H1" } else { "true" },
                    100.0 * noise
                );
                println!("{}", header());
                for r in &rows {
                    let mut resp = |d: &[Vec<f64>], s: f64| (c.respond)(d, s);
                    if let Some((o, pick)) =
                        run_row(r, &setup, None, &warp0, &mut resp, &LADDER_DB, noise)
                    {
                        print_outcome(r.name, &o, &pick);
                    }
                }
            }
        }
    }
}

/// Combinations: the warp under the plain secant (the tracked-prior form),
/// the warp alone, the predicted step, and the starter pulse placed by the
/// lever rule.
fn part_combos() {
    let rows = [
        row("shape warp alone", Kind::Shape, Fol::WarpNoSecant),
        row("shape warp+secant (noise ridge)", Kind::Shape, Fol::Warp),
        row("shape warp+plain secant", Kind::Shape, Fol::WarpPlainSecant),
        row("inverse warp alone", Kind::Inverse, Fol::WarpNoSecant),
        row("inverse warp+secant", Kind::Inverse, Fol::Warp),
        row(
            "inverse warp+plain secant",
            Kind::Inverse,
            Fol::WarpPlainSecant,
        ),
        Row {
            predicted_step: true,
            ..row("inverse warp+secant, step", Kind::Inverse, Fol::Warp)
        },
        Row {
            predicted_step: true,
            ..row(
                "inverse warp+plain secant, step",
                Kind::Inverse,
                Fol::WarpPlainSecant,
            )
        },
    ];
    let starter_rows = [row("shape warp+secant", Kind::Shape, Fol::Warp)];
    for key in ["8", "close", "dense"] {
        let c = case(key);
        let setup0 = &c.setup_h1;
        let warp0 = base_warp(setup0);
        let setup = with_polarity(setup0, true);
        for noise in [0.0, 0.30] {
            println!(
                "\n== {}; identification H1; targets mixed; noise {:.0}%",
                c.name,
                100.0 * noise
            );
            println!("{}", header());
            for r in &rows {
                let mut resp = |d: &[Vec<f64>], s: f64| (c.respond)(d, s);
                if let Some((o, pick)) = run_row(
                    r,
                    &setup,
                    c.modal_h1.as_ref(),
                    &warp0,
                    &mut resp,
                    &LADDER_DB,
                    noise,
                ) {
                    print_outcome(r.name, &o, &pick);
                }
            }
            // Starter pulses: none, at -18 dB (lever 4.8), and at the level
            // where the lever (s_first - s_seed) / (s_seed - s_id) is 1.5.
            let s_first = level_scale(LADDER_DB[0]);
            let s_seed = (s_first + 1.5 * S_ID) / 2.5;
            let lever_db = 20.0 * s_seed.log10();
            for (label, starter) in [
                ("starter -18 dB", -18.0),
                ("starter by lever 1.5", lever_db),
            ] {
                let ladder: Vec<f64> = std::iter::once(starter).chain(LADDER_DB).collect();
                let mut resp = |d: &[Vec<f64>], s: f64| (c.respond)(d, s);
                let (mut law, _) = build(&starter_rows[0], &setup, None, &warp0).expect("law");
                let rows = run_research_ladder(
                    law.as_mut(),
                    &setup,
                    &mut resp,
                    &ladder,
                    noise,
                    SEED,
                    None,
                )
                .expect("run");
                println!(
                    "  {label:24} ({starter:+.1} dB): starter {:+5.1}, -12 dB {:+5.1}, -9 {:+5.1}, first 0 dB {:+5.1} ({:.1}), p8 {:.1}, p14 {:.1}",
                    rows[0].over_db,
                    rows[1].over_db,
                    rows[2].over_db,
                    rows[5].over_db,
                    rows[5].worst_db,
                    rows[8].worst_db,
                    rows[14].worst_db
                );
            }
        }
    }
}

/// The warp's cost natively: setup, then observe and next model a pulse.
fn part_cost() {
    for (name, plant) in [
        ("4 x 4", LevelPlant::four_by_four_damped(0.08, 0.0, 0.02)),
        ("16 x 8", LevelPlant::sixteen_by_eight(0.08, 0.0, 11)),
    ] {
        let truth = plant.modal_truth(S_ID);
        let h = identify_h1_model(&truth, 16, 0.05, ID_SEED).expect("identify");
        let setup = research_setup_signed(h.clone(), &vec![1.0; plant.n_c]);
        let t0 = Instant::now();
        let mut w = base_warp(&setup);
        let t_setup = t0.elapsed().as_secs_f64();
        let mut law = ShapeLaw::new(setup.clone(), ShapeParams::default()).unwrap();
        let out = law.seed(-12.0).unwrap();
        let mut times = Vec::new();
        for (k, level) in [-12.0, -12.0, -12.0, -9.0, -6.0, -3.0, 0.0, 0.0, 0.0]
            .iter()
            .enumerate()
        {
            let s = level_scale(*level);
            let drv: Vec<Vec<f64>> = out
                .drives
                .iter()
                .map(|d| d.iter().map(|v| v * s / level_scale(-12.0)).collect())
                .collect();
            let y = plant.respond(&drv, s, FS_HZ).unwrap();
            let d: Vec<Vec<C64>> = drv
                .iter()
                .map(|x| rfft(x).unwrap().into_iter().map(|v| v / s).collect())
                .collect();
            let yy: Vec<Vec<C64>> = y
                .iter()
                .map(|x| rfft(x).unwrap().into_iter().map(|v| v / s).collect())
                .collect();
            let next = [-12.0, -12.0, -9.0, -6.0, -3.0, 0.0, 0.0, 0.0, 0.0][k];
            let t1 = Instant::now();
            w.observe(&Observation {
                level_db: *level,
                drives: &d,
                responses: &yy,
            })
            .unwrap();
            w.model_for(next).unwrap();
            times.push(t1.elapsed().as_secs_f64());
        }
        let worst = times.iter().fold(0.0f64, |a, &b| a.max(b));
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        println!(
            "{name}: warp setup {t_setup:.2} s, bank {:.0} MB; observe and next model {:.0} ms mean, {:.0} ms worst (up to 3 pulses at a level)",
            w.bank_bytes() as f64 / 1e6,
            1e3 * mean,
            1e3 * worst
        );
    }
}

/// The warp with the secant on the 4 x 4 at 8%, exact identification, noise
/// 30%, over noise seeds 1 to 8 (for comparison with another implementation
/// whose noise generator differs).
fn part_seeds() {
    let c = case("8");
    let warp0 = base_warp(&c.setup_true);
    let r = row("shape warp+secant", Kind::Shape, Fol::Warp);
    for mixed in [false, true] {
        let setup = with_polarity(&c.setup_true, mixed);
        let mut acc = [0.0f64; 3];
        for seed in 1..=8u64 {
            let (mut law, _) = build(&r, &setup, None, &warp0).expect("law");
            let mut resp = |d: &[Vec<f64>], s: f64| (c.respond)(d, s);
            let rows = run_research_ladder(
                law.as_mut(),
                &setup,
                &mut resp,
                &LADDER_DB,
                0.30,
                seed,
                None,
            )
            .expect("run");
            println!(
                "{} seed {seed}: p5 over {:+.2} p8 {:.2} p14 {:.2}",
                if mixed { "mixed" } else { "in phase" },
                rows[4].over_db,
                rows[7].worst_db,
                rows[13].worst_db
            );
            acc[0] += rows[4].over_db / 8.0;
            acc[1] += rows[7].worst_db / 8.0;
            acc[2] += rows[13].worst_db / 8.0;
        }
        println!(
            "{} mean: p5 over {:+.2} p8 {:.2} p14 {:.2}",
            if mixed { "mixed" } else { "in phase" },
            acc[0],
            acc[1],
            acc[2]
        );
    }
}

/// Whether a follower inside the choice behaves as it does alone: the
/// choice held on one follower (margins too large to cross) against each
/// follower alone and the choice, on one plant (dense by default), true
/// identification, in phase, clean, with the over-test of every pulse.
fn part_diag(key: &str) {
    let c = case(key);
    let setup = with_polarity(&c.setup_true, false);
    let warp0 = base_warp(&setup);
    for (label, start) in [("held on the warp", 0usize), ("held on tracking", 1)] {
        let wp: Box<dyn ModelProvider> = Box::new(warp0.clone());
        let tp = track(&c.modal_true, &setup.plant.frf).expect("track");
        let ch = ChoiceModel::new(
            wp,
            tp,
            fit_band(),
            FS_HZ,
            ChoiceOptions {
                ratio: 1e9,
                z: 1e9,
                start,
                ..Default::default()
            },
        )
        .expect("choice");
        let log = ch.log();
        let mut law = ShapeLaw::with_options(
            setup.clone(),
            ShapeParams::default(),
            ShapeExtras {
                provider: Some(Box::new(ch)),
                safety: ShapeSafety::off(),
                robust_first: None,
            },
        )
        .expect("law");
        let mut resp = |d: &[Vec<f64>], s: f64| (c.respond)(d, s);
        let rows = run_research_ladder(&mut law, &setup, &mut resp, &LADDER_DB, 0.0, SEED, None)
            .expect("run");
        let l = log.lock().expect("log");
        println!(
            "{label}: over by pulse {}",
            rows.iter()
                .map(|x| format!("{:+.2}", x.over_db))
                .collect::<Vec<_>>()
                .join(" ")
        );
        println!(
            "{label}: p5 over {:+.1}, p8 {:.1}; residuals (warp, tracking) by pulse: {}",
            rows[4].over_db,
            rows[7].worst_db,
            l.iter()
                .map(|r| format!("{:.3}/{:.3}", r.residuals[0][0], r.residuals[0][1]))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    for r in [
        row("shape warp+secant", Kind::Shape, Fol::Warp),
        row("shape track+secant", Kind::Shape, Fol::Track),
        row("shape choice", Kind::Shape, Fol::Choice),
    ] {
        let (mut law, log) = build(&r, &setup, Some(&c.modal_true), &warp0).expect("law");
        let mut resp = |d: &[Vec<f64>], s: f64| (c.respond)(d, s);
        let rows =
            run_research_ladder(law.as_mut(), &setup, &mut resp, &LADDER_DB, 0.0, SEED, None)
                .expect("run");
        let res = log
            .map(|l| {
                l.lock()
                    .expect("log")
                    .iter()
                    .map(|r| format!("{:.3}/{:.3}", r.residuals[0][0], r.residuals[0][1]))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        println!(
            "{}: over by pulse {}",
            r.name,
            rows.iter()
                .map(|x| format!("{:+.2}", x.over_db))
                .collect::<Vec<_>>()
                .join(" ")
        );
        println!(
            "{}: p5 over {:+.1}; residual of the model in force by pulse {} {res}",
            r.name,
            rows[4].over_db,
            rows.iter()
                .map(|x| format!("{:.3}", x.value("model_prediction_residual")))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let part = args.next().unwrap_or_else(|| "crosscheck".into());
    let plant = args.next();
    match part.as_str() {
        "crosscheck" => part_crosscheck(),
        "compare" => part_compare(plant.as_deref()),
        "gate" => part_gate(),
        "combos" => part_combos(),
        "cost" => part_cost(),
        "seeds" => part_seeds(),
        "diag" => part_diag(plant.as_deref().unwrap_or("dense")),
        other => eprintln!("unknown part {other}"),
    }
}
