//! Research round 3: the jointed plants, whose modes soften at the peak of a
//! pulse and recover in its ring-down. Studies only (ignored, run on demand;
//! each prints its table).
//!
//! Two harnesses. The direct one takes the engine's own identification (the
//! engine run to the end of `identify`, station seed 11) and then plays drives
//! straight through the plant's noise-free simulation, so one pulse can be
//! studied at any level, under any model, without a ladder. The engine one is
//! the whole run through the host firer, as package X's studies.
//!
//! Requirements R3-A to R3-D and the expectations were fixed in the research
//! record's experiment log before this file was written.

// The studies index several per-mode and per-drive arrays together, and carry
// their rows as plain tuples.
#![allow(clippy::needless_range_loop, clippy::type_complexity)]

mod support;

use hertzforge_hal_sim::jointed::JointedPlant;
use hertzforge_hal_sim::plants::ShockPlant;
use hertzforge_mimo_shock::builtin::math_setup;
use hertzforge_mimo_shock::{LadderPlan, MimoShockLawFactory, TargetSpec};
use hertzforge_mimo_shock_law_math as math;
use math::first_pulse::{
    added_damping_model, coded_frf, equivalent_model, family_inverse, stretched_family,
};
use math::provider::FrozenModel;
use math::reference_plant::{Rng, FS_HZ as RFS, RECORD_LEN as RLEN};
use math::research_plant::{h1_from_records, identify_h1_model, research_setup_signed, LevelPlant};
use math::shape::{ShapeExtras, ShapeLaw, ShapeParams, ShapeSafety};
use math::spectrum::{apply, rfft, rows_irfft, rows_rfft};
use math::srs::{error_db, srs_rows};
use math::warp::{WarpModel, WarpOptions};
use math::{level_scale, LawSteps, MatrixStack, C64};
use std::sync::Arc;
use support::*;

// ---------------------------------------------------------------------------
// The direct harness.
// ---------------------------------------------------------------------------

/// A plant as a pulse sees it: drives (already at the level) and the level's
/// amplitude scale in, responses out.
type Respond = Box<dyn Fn(&[Vec<f64>], f64) -> Vec<Vec<f64>>>;

struct Bench {
    name: String,
    ms: math::Setup,
    respond: Respond,
    jointed: Option<JointedPlant>,
    warp: WarpModel,
}

fn warp_for(ms: &math::Setup) -> WarpModel {
    let mask = math::drive_mask(ms);
    WarpModel::new(ms.plant.frf.clone(), mask, ms.fs_hz, warp_options(ms)).expect("a warp")
}

/// The warp's options: package W's, with the bands moved under the shock
/// plants' Nyquist frequency (6.4 kHz) and the band's bottom under their
/// first mode (72 Hz).
fn warp_options(ms: &math::Setup) -> WarpOptions {
    if ms.fs_hz < 15_000.0 {
        WarpOptions {
            band_lo_hz: 40.0,
            band_hi_hz: 2400.0,
            bank_hi_hz: 3000.0,
            delay_band_hz: (2500.0, 4000.0),
            noise_band_hz: (4000.0, 6000.0),
            ..WarpOptions::default()
        }
    } else {
        WarpOptions::default()
    }
}

/// The engine's identification of `plant` under `spec`, and the plant itself.
fn jointed_bench(plant: ShockPlant, spec: TargetSpec, len: usize) -> Bench {
    let (nc, nd) = plant.dims();
    let cfg = config(nc, 0, nd, len);
    let mut e = hertzforge_mimo_shock::MimoShockEngine::new(
        cfg,
        Arc::new(hertzforge_mimo_shock::builtin::ShapeFactory::default()),
    )
    .expect("a configuration")
    .with_target_spec(spec)
    .with_clock(Arc::new(ManualClock::default()))
    .with_pacing(false);
    e.arm(
        station(plant, FS, 11),
        table(nc, 0, nd),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .expect("armed");
    e.identify().expect("identified");
    let ms = math_setup(e.setup().expect("a setup"));
    let jp = plant.plant();
    let jp2 = jp.clone();
    Bench {
        name: plant.name().into(),
        warp: warp_for(&ms),
        ms,
        respond: Box::new(move |d, _| jp2.simulate(FS, d).expect("simulated").outputs),
        jointed: Some(jp),
    }
}

/// The research 4 x 4 at 8 %, mixed polarity, H1 identification at scale 0.1
/// (16 records, 5 % noise, seed 7: the research rounds' settings).
fn research_bench() -> Bench {
    let p = LevelPlant::four_by_four_damped(0.08, 0.0, 0.02);
    let h1 = identify_h1_model(&p.modal_truth(0.1), 16, 0.05, 7).expect("identified");
    let pol: Vec<f64> = (0..p.n_c)
        .map(|c| if c % 2 == 1 { -1.0 } else { 1.0 })
        .collect();
    let ms = research_setup_signed(h1, &pol);
    Bench {
        name: "research-4x4-8%".into(),
        warp: warp_for(&ms),
        ms,
        respond: Box::new(move |d, s| p.respond(d, s, RFS).expect("responded")),
        jointed: None,
    }
}

impl Bench {
    fn mask(&self) -> Vec<bool> {
        math::drive_mask(&self.ms)
    }

    /// `shape`'s first drive at `level_db` on `model` (frozen).
    fn seed(&self, model: &MatrixStack, level_db: f64) -> Vec<Vec<f64>> {
        self.law(model).seed(level_db).expect("seeded").drives
    }

    fn law(&self, model: &MatrixStack) -> ShapeLaw {
        ShapeLaw::with_options(
            self.ms.clone(),
            ShapeParams::default(),
            ShapeExtras {
                provider: Some(Box::new(FrozenModel::new(model.clone(), self.mask()))),
                safety: ShapeSafety::off(),
                robust_first: None,
            },
        )
        .expect("a law")
    }

    /// `(over-test, worst |error|)` of responses `y` at `level_db`, dB.
    fn judge(&self, y: &[Vec<f64>], level_db: f64) -> (f64, f64) {
        let t = self.ms.srs.as_ref().expect("an SRS target");
        let m = srs_rows(y, self.ms.fs_hz, &t.natural_hz, t.q, level_scale(level_db));
        let (mut over, mut worst) = (f64::NEG_INFINITY, 0.0f64);
        for (mr, tr) in m.iter().zip(&t.points) {
            for e in error_db(mr, tr) {
                over = over.max(e);
                worst = worst.max(e.abs());
            }
        }
        (over, worst)
    }

    /// Natural frequency of the SRS point with the largest over-test.
    fn over_at_hz(&self, y: &[Vec<f64>], level_db: f64) -> f64 {
        let t = self.ms.srs.as_ref().expect("an SRS target");
        let m = srs_rows(y, self.ms.fs_hz, &t.natural_hz, t.q, level_scale(level_db));
        let mut best = (f64::NEG_INFINITY, 0.0);
        for (mr, tr) in m.iter().zip(&t.points) {
            for (k, e) in error_db(mr, tr).into_iter().enumerate() {
                if e > best.0 {
                    best = (e, t.natural_hz[k]);
                }
            }
        }
        best.1
    }

    fn play(&self, d: &[Vec<f64>], level_db: f64) -> Vec<Vec<f64>> {
        (self.respond)(d, level_scale(level_db))
    }
}

/// `model` applied to `drives` (circular), the predicted responses.
fn predict(model: &MatrixStack, drives: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = drives[0].len();
    let d = rows_rfft(drives, 1.0).expect("spectra");
    rows_irfft(&apply(model, &d), n, 1.0).expect("records")
}

fn rel(a: &[Vec<f64>], b: &[Vec<f64>]) -> f64 {
    let (mut num, mut den) = (0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        for (u, v) in x.iter().zip(y) {
            num += (u - v) * (u - v);
            den += u * u;
        }
    }
    (num / den.max(1e-300)).sqrt()
}

fn freqs(ms: &math::Setup) -> Vec<f64> {
    let df = ms.fs_hz / ms.record_len as f64;
    (0..=ms.record_len / 2).map(|k| k as f64 * df).collect()
}

/// Index of the largest absolute sample over every record.
fn peak_index(y: &[Vec<f64>]) -> usize {
    let mut best = (0.0, 0);
    for r in y {
        for (i, v) in r.iter().enumerate() {
            if v.abs() > best.0 {
                best = (v.abs(), i);
            }
        }
    }
    best.1
}

/// A weighted norm's weight: one up to `t0`, `exp(-b (t - t0))` after it, so
/// the loud part counts and the ring-down fades. (A metric only: unlike
/// [`exp_weight`] over the whole record it does not keep a convolution exact.)
fn loud_weight(rows: &[Vec<f64>], fs: f64, b: f64, t0: f64) -> Vec<Vec<f64>> {
    rows.iter()
        .map(|r| {
            r.iter()
                .enumerate()
                .map(|(i, v)| v * (-b * (i as f64 / fs - t0).max(0.0)).exp())
                .collect()
        })
        .collect()
}

/// The best linear model of the jointed plant's own form for one pulse: every
/// mode's envelope frozen, chosen to minimize the (weighted) misfit. A common
/// ratio on a grid, then two coordinate sweeps of each mode's own ratio.
/// Returns `(misfit, ratios)`. `weight`: `(rate per s, t0 s)` or none.
fn best_frozen(
    jp: &JointedPlant,
    ms: &math::Setup,
    d: &[Vec<f64>],
    y: &[Vec<f64>],
    weight: Option<(f64, f64)>,
) -> (f64, Vec<f64>) {
    let fr = freqs(ms);
    let w = |r: &[Vec<f64>]| match weight {
        Some((b, t0)) => loud_weight(r, ms.fs_hz, b, t0),
        None => r.to_vec(),
    };
    let yw = w(y);
    let cost = |ratios: &[f64]| -> f64 {
        let env: Vec<f64> = jp
            .modes
            .iter()
            .zip(ratios)
            .map(|(m, r)| r * m.reference_amplitude)
            .collect();
        let h = jp.frf_at_envelope(&fr, ms.fs_hz, &env);
        rel(&yw, &w(&predict(&h, d)))
    };
    let nm = jp.modes.len();
    let grid: Vec<f64> = (0..=24)
        .map(|k| {
            if k == 0 {
                0.0
            } else {
                0.02 * 1.25f64.powi(k - 1)
            }
        })
        .collect();
    let mut best = (f64::INFINITY, vec![0.0; nm]);
    for &g in &grid {
        let r = vec![g; nm];
        let c = cost(&r);
        if c < best.0 {
            best = (c, r);
        }
    }
    for _ in 0..2 {
        for m in 0..nm {
            for &g in &grid {
                let mut r = best.1.clone();
                r[m] = g;
                let c = cost(&r);
                if c < best.0 {
                    best = (c, r);
                }
            }
        }
    }
    best
}

fn jointed_benches() -> Vec<(ShockPlant, Bench)> {
    [
        ShockPlant::Linear4x4,
        ShockPlant::Jointed4x4Moderate,
        ShockPlant::Jointed4x4Severe,
    ]
    .into_iter()
    .map(|p| (p, jointed_bench(p, srs_twist(), LEN_SRS)))
    .collect()
}

const LEVELS: [f64; 15] = [
    -40.0, -36.0, -32.0, -28.0, -24.0, -21.0, -18.0, -16.0, -14.0, -12.0, -9.0, -6.0, -3.0, -1.5,
    0.0,
];

/// MS-R3-01 and -02: the frozen first pulse at each level (its over-test,
/// absolute level, where the over-test sits, the model's mismatch), and the
/// best linear model's misfit (plain and exponentially weighted), with where
/// its residual sits in time and frequency.
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r3_study_first_pulse_by_level() {
    let mut benches: Vec<Bench> = jointed_benches().into_iter().map(|b| b.1).collect();
    benches.push(research_bench());
    for b in &benches {
        println!("\n{}: the frozen first pulse by level\n", b.name);
        println!("| level | over | absolute | at Hz | mismatch | env1 | best | best w20 | best w60 | resid loud | resid ring | top mode band |");
        println!("|---|---|---|---|---|---|---|---|---|---|---|---|");
        let h = b.ms.plant.frf.clone();
        for &l in &LEVELS {
            let d = b.seed(&h, l);
            let y = b.play(&d, l);
            let (over, _) = b.judge(&y, l);
            let mism = rel(&y, &predict(&h, &d));
            let at = b.over_at_hz(&y, l);
            let mut cols = String::from("- | - | - | - | - | - | -");
            let mut env1 = f64::NAN;
            if let Some(jp) = &b.jointed {
                let sim = jp.simulate(FS, &d).expect("simulated");
                env1 = sim.peak_envelope[0] / jp.modes[0].reference_amplitude;
                let t_pk = peak_index(&y) as f64 / FS;
                let (m0, r) = best_frozen(jp, &b.ms, &d, &y, None);
                let (m20, _) = best_frozen(jp, &b.ms, &d, &y, Some((20.0, t_pk)));
                let (m60, _) = best_frozen(jp, &b.ms, &d, &y, Some((60.0, t_pk)));
                // Where the best plain model's residual sits.
                let fr = freqs(&b.ms);
                let env: Vec<f64> = jp
                    .modes
                    .iter()
                    .zip(&r)
                    .map(|(m, x)| x * m.reference_amplitude)
                    .collect();
                let yh = predict(&jp.frf_at_envelope(&fr, FS, &env), &d);
                let ipk = peak_index(&y);
                let (lo, hi) = (
                    ipk.saturating_sub((0.01 * FS) as usize),
                    ipk + (0.03 * FS) as usize,
                );
                let (mut loud, mut ring, mut tot) = (0.0, 0.0, 0.0);
                let mut res = Vec::new();
                for (yr, hr) in y.iter().zip(&yh) {
                    let e: Vec<f64> = yr.iter().zip(hr).map(|(a, c)| a - c).collect();
                    for (i, v) in e.iter().enumerate() {
                        let p = v * v;
                        tot += p;
                        if (lo..hi).contains(&i) {
                            loud += p;
                        } else if i >= hi {
                            ring += p;
                        }
                    }
                    res.push(e);
                }
                let spec = rows_rfft(&res, 1.0).expect("spectra");
                let df = FS / b.ms.record_len as f64;
                let mut band_e: Vec<f64> = vec![0.0; jp.modes.len()];
                let mut all_e = 0.0;
                for row in &spec {
                    for (k, v) in row.iter().enumerate() {
                        let f = k as f64 * df;
                        let p = v.norm_sqr();
                        all_e += p;
                        for (m, md) in jp.modes.iter().enumerate() {
                            if (f - md.f_hz).abs() <= 0.1 * md.f_hz {
                                band_e[m] += p;
                            }
                        }
                    }
                }
                let (bm, be) =
                    band_e
                        .iter()
                        .enumerate()
                        .fold((0, 0.0), |a, (m, v)| if *v > a.1 { (m, *v) } else { a });
                cols = format!(
                    "{m0:.3} | {m20:.3} | {m60:.3} | {:.2} | {:.2} | {:.0} Hz {:.2}",
                    loud / tot,
                    ring / tot,
                    jp.modes[bm].f_hz,
                    be / all_e.max(1e-300)
                );
            }
            println!(
                "| {l:+5.1} | {over:+6.2} | {:+6.2} | {at:6.0} | {mism:.3} | {env1:.2} | {cols} |",
                l + over
            );
        }
    }
}

// ---------------------------------------------------------------------------
// First-pulse models.
// ---------------------------------------------------------------------------

/// The family model: the equivalent model of the family inverse over the
/// identification lowered by `0, drop/2, drop`.
fn family_model(b: &Bench, drop: f64) -> MatrixStack {
    let fam = stretched_family(&b.warp, &[0.0, 0.5 * drop, drop]);
    let mask = b.mask();
    let g = family_inverse(&fam, &[1.0, 1.0, 1.0], &mask, 1e-3).expect("a family inverse");
    equivalent_model(&g, &b.ms.plant.frf, &mask).expect("an equivalent model")
}

/// Band-limited (5 Hz to 2.5 kHz) random drives, `frames` half-overlapped
/// frames long, unit rms before the band limit.
fn random_drives(nd: usize, len: usize, fs: f64, seed: u64) -> Vec<Vec<f64>> {
    let mut rng = Rng::new(seed);
    (0..nd)
        .map(|_| {
            let x: Vec<f64> = (0..len).map(|_| rng.normal()).collect();
            let mut s = rfft(&x).expect("a spectrum");
            let df = fs / len as f64;
            for (k, v) in s.iter_mut().enumerate() {
                let f = k as f64 * df;
                if !(5.0..=2500.0).contains(&f) {
                    *v = C64::new(0.0, 0.0);
                }
            }
            math::spectrum::irfft(&s, len).expect("a record")
        })
        .collect()
}

fn rms_rows(r: &[Vec<f64>]) -> Vec<f64> {
    r.iter()
        .map(|x| (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt())
        .collect()
}

/// H1 from random drives played through the jointed plant with the control
/// nearest its mark `below_db` under its reference's rms (the engine's rule),
/// 40 half-overlapped frames. Returns the model and the drive rms, V.
fn random_id(b: &Bench, below_db: f64) -> (MatrixStack, f64) {
    let n = b.ms.record_len;
    let len = n * 32;
    let nd = b.ms.n_drives;
    let unit = random_drives(nd, len, b.ms.fs_hz, 99);
    let y1 = b.play(&unit, 0.0);
    let ry = rms_rows(&y1);
    let rr = rms_rows(&b.ms.reference);
    // Scale so the control closest to its mark sits `below_db` under it.
    let k = ry
        .iter()
        .zip(&rr)
        .map(|(y, r)| r * level_scale(-below_db) / y)
        .fold(f64::INFINITY, f64::min);
    let drives: Vec<Vec<f64>> = unit
        .iter()
        .map(|r| r.iter().map(|v| v * k).collect())
        .collect();
    let y = b.play(&drives, 0.0);
    let h = h1_from_records(&drives, &y, n).expect("an H1");
    (h, k * rms_rows(&unit)[0])
}

/// The four-shot Hadamard codes.
fn hadamard4() -> Vec<Vec<f64>> {
    vec![
        vec![1.0, 1.0, 1.0, 1.0],
        vec![1.0, -1.0, 1.0, -1.0],
        vec![1.0, 1.0, -1.0, -1.0],
        vec![1.0, -1.0, -1.0, 1.0],
    ]
}

/// The plant from four coded copies of the frozen first drive at `level_db`.
fn coded_id(b: &Bench, level_db: f64) -> MatrixStack {
    let d = b.seed(&b.ms.plant.frf, level_db);
    let codes = hadamard4();
    let shots: Vec<Vec<Vec<C64>>> = codes
        .iter()
        .map(|c| {
            let dc: Vec<Vec<f64>> = d
                .iter()
                .zip(c)
                .map(|(r, s)| r.iter().map(|v| v * s).collect())
                .collect();
            rows_rfft(&b.play(&dc, level_db), 1.0).expect("spectra")
        })
        .collect();
    let ds = rows_rfft(&d, 1.0).expect("spectra");
    coded_frf(&ds, &shots, &codes, &b.ms.plant.frf, 1e-2).expect("a coded model")
}

/// MS-R3-03: the first pulse under each first-pulse method.
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r3_study_first_pulse_methods() {
    let mut benches: Vec<Bench> = jointed_benches().into_iter().map(|b| b.1).collect();
    benches.push(research_bench());
    for b in &benches {
        let h = b.ms.plant.frf.clone();
        let mut models: Vec<(String, MatrixStack)> =
            vec![("frozen (identification)".into(), h.clone())];
        // Added damping: about +0.02 and +0.06 of damping ratio at 72 Hz.
        for dz in [0.02, 0.06] {
            models.push((
                format!("added damping {dz:.2} at 72 Hz"),
                added_damping_model(&b.warp, dz * std::f64::consts::TAU * 72.0),
            ));
        }
        for drop in [0.04, 0.08, 0.12] {
            models.push((
                format!("family to {:.0} %", 100.0 * drop),
                family_model(b, drop),
            ));
        }
        if let Some(jp) = &b.jointed {
            // The notch limit on the true small-signal modes (no warp): each
            // mode's damping ratio raised by `dz`.
            for dz in [0.02, 0.06] {
                let mut lin = jp.linear();
                for m in lin.modes.iter_mut() {
                    m.zeta += dz;
                }
                let env = vec![0.0; lin.modes.len()];
                models.push((
                    format!("added damping {dz:.2} on every true mode (exact)"),
                    lin.frf_at_envelope(&freqs(&b.ms), FS, &env),
                ));
            }
            for below in [10.0, 0.0] {
                let (m, v) = random_id(b, below);
                models.push((
                    format!(
                        "random id {below:.0} dB under rms ({v:.2} V rms, {:.1} s of excitation)",
                        32.0 * b.ms.record_len as f64 / b.ms.fs_hz
                    ),
                    m,
                ));
            }
        }
        println!(
            "\n{}: the first pulse by method (over / absolute, dB)\n",
            b.name
        );
        println!("| method | -24 | -18 | -12 |");
        println!("|---|---|---|---|");
        for (name, m) in &models {
            let cells: Vec<String> = [-24.0, -18.0, -12.0]
                .iter()
                .map(|&l| {
                    let (o, _) = b.judge(&b.play(&b.seed(m, l), l), l);
                    format!("{o:+6.2} / {:+6.2}", l + o)
                })
                .collect();
            println!("| {name} | {} |", cells.join(" | "));
        }
        // Coded copies 12 dB under the first level (four pulses there).
        let cells: Vec<String> = [-24.0, -18.0, -12.0]
            .iter()
            .map(|&l| {
                let m = coded_id(b, l - 12.0);
                let (o, _) = b.judge(&b.play(&b.seed(&m, l), l), l);
                format!("{o:+6.2} / {:+6.2}", l + o)
            })
            .collect();
        println!(
            "| coded copies 12 dB under (4 pulses) | {} |",
            cells.join(" | ")
        );
    }
    let _ = RLEN;
}

// ---------------------------------------------------------------------------
// The engine harness.
// ---------------------------------------------------------------------------

/// `shape` on a model made from the identification by `make` (frozen, the
/// built-in law's math with no safety options unless `clamp_db` is set).
struct ModelShape {
    make: fn(&math::Setup) -> MatrixStack,
    clamp_db: Option<f64>,
    /// The warp follower (package W's, noise-ridged secant) started on the
    /// model, in place of the frozen model.
    warp: bool,
    /// The family provider over the identification lowered by these drops
    /// (equal priors, temperature 0.1), and whether it follows the evidence
    /// within a level; `make` is then unused.
    adaptive: Option<(&'static [f64], bool)>,
}

struct Adapter {
    law: ShapeLaw,
    len: usize,
    nf: usize,
}

const KEYS: [&str; 1] = ["worst_srs_error_db"];

fn outs(o: math::PulseOutputs) -> hertzforge_mimo_shock::PulseOutputs {
    hertzforge_mimo_shock::PulseOutputs {
        drives: o.drives.concat(),
        delays_s: o.delays_s,
        values: o
            .values
            .into_iter()
            .filter(|(k, _)| KEYS.contains(k))
            .map(|(k, v)| hertzforge_shock_common::law_guard::LawValue::new(k, v))
            .collect(),
    }
}

impl hertzforge_mimo_shock::MimoShockLaw for Adapter {
    fn name(&self) -> &str {
        "shape-model"
    }
    fn seed(
        &mut self,
        level_db: f64,
    ) -> Result<hertzforge_mimo_shock::PulseOutputs, hertzforge_shock_common::law_guard::LawFault>
    {
        self.law
            .seed(level_db)
            .map(outs)
            .map_err(hertzforge_shock_common::law_guard::LawFault::Failed)
    }
    fn iterate(
        &mut self,
        i: &hertzforge_mimo_shock::PulseInputs,
    ) -> Result<hertzforge_mimo_shock::PulseOutputs, hertzforge_shock_common::law_guard::LawFault>
    {
        let rows = |f: &[f64], n: usize| f.chunks(n.max(1)).map(|c| c.to_vec()).collect();
        let m = math::PulseInputs {
            pulse_index: i.pulse_index as usize - 1,
            level_db: i.level_db,
            next_level_db: i.next_level_db,
            drives: rows(&i.drives, self.len),
            responses: rows(&i.responses, self.len),
            srs: i.srs.as_ref().map(|s| rows(s, self.nf)),
        };
        self.law
            .iterate(&m)
            .map(outs)
            .map_err(hertzforge_shock_common::law_guard::LawFault::Failed)
    }
}

impl MimoShockLawFactory for ModelShape {
    fn build(
        &self,
        setup: &hertzforge_mimo_shock::Setup,
    ) -> Result<
        Box<dyn hertzforge_mimo_shock::MimoShockLaw + Send>,
        hertzforge_shock_common::law_guard::LawFault,
    > {
        let ms = math_setup(setup);
        let model = (self.make)(&ms);
        let mask = math::drive_mask(&ms);
        let mut safety = ShapeSafety::off();
        safety.gain_clamp_db = self.clamp_db;
        let nf = ms.srs.as_ref().map(|t| t.natural_hz.len()).unwrap_or(0);
        let (fs, wopts, frf) = (ms.fs_hz, warp_options(&ms), ms.plant.frf.clone());
        let law = ShapeLaw::with_options(
            ms,
            ShapeParams::default(),
            ShapeExtras {
                provider: Some(if let Some((drops, within)) = self.adaptive {
                    let w = WarpModel::new(frf.clone(), mask.clone(), fs, wopts.clone())
                        .map_err(hertzforge_shock_common::law_guard::LawFault::Failed)?;
                    Box::new(
                        math::first_pulse::FamilyModel::new(
                            frf.clone(),
                            mask.clone(),
                            stretched_family(&w, drops),
                            drops.to_vec(),
                            vec![1.0; drops.len()],
                            1e-3,
                            0.1,
                            within,
                        )
                        .map_err(hertzforge_shock_common::law_guard::LawFault::Failed)?,
                    ) as Box<dyn math::provider::ModelProvider>
                } else if self.warp {
                    Box::new(
                        WarpModel::new(model, mask, fs, wopts)
                            .map_err(hertzforge_shock_common::law_guard::LawFault::Failed)?,
                    ) as Box<dyn math::provider::ModelProvider>
                } else {
                    Box::new(FrozenModel::new(model, mask))
                }),
                safety,
                robust_first: None,
            },
        )
        .map_err(hertzforge_shock_common::law_guard::LawFault::Failed)?;
        Ok(Box::new(Adapter {
            law,
            len: setup.record_len,
            nf,
        }))
    }
    fn describe(&self) -> hertzforge_shock_common::law_guard::LawDescriptor {
        hertzforge_mimo_shock::builtin::ShapeFactory::default().describe()
    }
    fn manifest(&self) -> hertzforge_mimo_shock::LawManifest {
        hertzforge_mimo_shock::LawManifest {
            targets: vec![hertzforge_mimo_shock::TargetKind::Srs],
            values: KEYS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

fn identity_model(ms: &math::Setup) -> MatrixStack {
    ms.plant.frf.clone()
}

fn family_from_setup(ms: &math::Setup, drop: f64) -> MatrixStack {
    let w = warp_for(ms);
    let fam = stretched_family(&w, &[0.0, 0.5 * drop, drop]);
    let mask = math::drive_mask(ms);
    let g = family_inverse(&fam, &[1.0, 1.0, 1.0], &mask, 1e-3).expect("a family inverse");
    equivalent_model(&g, &ms.plant.frf, &mask).expect("an equivalent model")
}

fn family8(ms: &math::Setup) -> MatrixStack {
    family_from_setup(ms, 0.08)
}

fn family4(ms: &math::Setup) -> MatrixStack {
    family_from_setup(ms, 0.04)
}

/// One engine run's numbers.
#[derive(Debug, Clone)]
struct Run {
    p1: f64,
    worst_abs_below: f64,
    worst_first_over: f64,
    f1: Option<f64>,
    f3: Option<f64>,
    n: usize,
    end: String,
}

fn engine_run(
    plant: ShockPlant,
    law: Arc<dyn MimoShockLawFactory>,
    ladder: Vec<f64>,
    per_level: u32,
    scaled: bool,
) -> Run {
    let (nc, nd) = plant.dims();
    let mut cfg = config(nc, 0, nd, LEN_SRS);
    cfg.ladder = LadderPlan {
        levels_db: ladder,
        max_pulses_per_level: per_level,
        ..LadderPlan::default()
    };
    cfg.guards.scaled_aborts = scaled;
    cfg.guards.law.max_pulses = cfg.guards.law.max_pulses.max(200u64);
    let mut e = hertzforge_mimo_shock::MimoShockEngine::new(cfg, law)
        .expect("a good configuration")
        .with_target_spec(srs_twist())
        .with_clock(Arc::new(ManualClock::default()))
        .with_pacing(false);
    let seed: u64 = std::env::var("R3_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(11);
    e.arm(
        station(plant, FS, seed),
        table(nc, 0, nd),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .expect("armed");
    e.identify().expect("identified");
    if e.end().is_none() {
        if let Err(hertzforge_mimo_shock::EngineError::Unreachable { .. }) = e.proceed() {
            e.acknowledge_unreachable("study");
            e.proceed().expect("proceeded");
        }
    }
    if e.end().is_none() {
        let _ = e.run_to_end(None);
    }
    let p = e.pulses();
    let full: Vec<_> = p.iter().filter(|q| q.level_db == 0.0).collect();
    let worst = |q: &hertzforge_mimo_shock::PulseReport| {
        q.controls
            .iter()
            .fold(0.0f64, |a, c| a.max(c.verdict.error_db.abs()))
    };
    let mut worst_first = f64::NEG_INFINITY;
    let mut last_level = f64::NAN;
    for q in p.iter() {
        if q.level_db != last_level {
            worst_first = worst_first.max(q.over_test_db);
            last_level = q.level_db;
        }
    }
    Run {
        p1: p.first().map(|q| q.over_test_db).unwrap_or(f64::NAN),
        worst_abs_below: p
            .iter()
            .filter(|q| q.level_db < 0.0)
            .map(|q| q.level_db + q.over_test_db)
            .fold(f64::NEG_INFINITY, f64::max),
        worst_first_over: worst_first,
        f1: full.first().map(|q| q.over_test_db),
        f3: full.get(2).map(|q| worst(q)),
        n: p.len(),
        end: e
            .end()
            .map(|x| format!("{:?} {}", x.outcome, x.reason))
            .unwrap_or_else(|| "none".into()),
    }
}

fn opt(o: Option<f64>) -> String {
    o.map(|v| format!("{v:+6.2}"))
        .unwrap_or_else(|| "    - ".into())
}

fn print_run(label: &str, r: &Run) {
    println!(
        "| {label} | {:+6.2} | {:+6.2} | {:+6.2} | {} | {} | {} | {} |",
        r.p1,
        r.worst_first_over,
        r.worst_abs_below,
        opt(r.f1),
        opt(r.f3),
        r.n,
        r.end
    );
}

/// A ladder from `start` to 0 dB in `step` dB steps (0 dB always the last).
fn ladder(start: f64, step: f64) -> Vec<f64> {
    let mut v = Vec::new();
    let mut l = start;
    while l < -1e-9 {
        v.push(l);
        l += step;
    }
    v.push(0.0);
    v
}

fn run_header(title: &str) {
    println!("\n{title}\n");
    println!("| run | p1 | worst first over | worst absolute below full | f1 | f3 err | n | end |");
    println!("|---|---|---|---|---|---|---|---|");
}

/// MS-R3-04: the climb. The frozen `shape` from a low start in steps, by
/// start, step, pulses a level, the gain clamp and the scaled aborts; and the
/// family model beside it.
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r3_study_climb() {
    let plants: Vec<ShockPlant> = match std::env::var("R3_PLANT").as_deref() {
        Ok("moderate") => vec![ShockPlant::Jointed4x4Moderate],
        Ok("severe") => vec![ShockPlant::Jointed4x4Severe],
        Ok("linear") => vec![ShockPlant::Linear4x4],
        _ => vec![
            ShockPlant::Linear4x4,
            ShockPlant::Jointed4x4Moderate,
            ShockPlant::Jointed4x4Severe,
        ],
    };
    let frozen = |clamp: Option<f64>| -> Arc<dyn MimoShockLawFactory> {
        Arc::new(ModelShape {
            make: identity_model,
            clamp_db: clamp,
            warp: false,
            adaptive: None,
        })
    };
    for plant in plants {
        run_header(&format!("{}, twist SRS", plant.name()));
        let t0 = std::time::Instant::now();
        print_run(
            "frozen, default ladder, scaled aborts on (today)",
            &engine_run(
                plant,
                frozen(None),
                LadderPlan::default().levels_db,
                3,
                true,
            ),
        );
        for scaled in [false, true] {
            for (start, step) in [
                (-40.0, 3.0),
                (-40.0, 1.5),
                (-30.0, 3.0),
                (-24.0, 3.0),
                (-40.0, 6.0),
            ] {
                for per in [3u32, 6] {
                    let r = engine_run(plant, frozen(None), ladder(start, step), per, scaled);
                    print_run(
                        &format!(
                            "frozen from {start} by {step}, {per} a level, aborts {}",
                            if scaled { "on" } else { "off" }
                        ),
                        &r,
                    );
                }
            }
        }
        for per in [3u32, 6] {
            print_run(
                &format!("frozen clamp 12 from -40 by 3, {per} a level, aborts off"),
                &engine_run(plant, frozen(Some(12.0)), ladder(-40.0, 3.0), per, false),
            );
        }
        for (name, make) in [
            ("family 8 %", family8 as fn(&math::Setup) -> MatrixStack),
            ("family 4 %", family4),
        ] {
            for (lname, lad) in [
                ("default ladder", LadderPlan::default().levels_db),
                ("from -24 by 3", ladder(-24.0, 3.0)),
                ("from -40 by 3", ladder(-40.0, 3.0)),
            ] {
                for per in [3u32, 6] {
                    let law = Arc::new(ModelShape {
                        make,
                        clamp_db: None,
                        warp: false,
                        adaptive: None,
                    });
                    print_run(
                        &format!("{name}, {lname}, {per} a level, aborts off"),
                        &engine_run(plant, law, lad.clone(), per, false),
                    );
                }
            }
        }
        println!("({:.0} s)", t0.elapsed().as_secs_f64());
    }
}

fn family12(ms: &math::Setup) -> MatrixStack {
    family_from_setup(ms, 0.12)
}

/// Four members, 0 to 12 % in 4 % steps (spacing two damping ratios).
fn family12x4(ms: &math::Setup) -> MatrixStack {
    let w = warp_for(ms);
    let fam = stretched_family(&w, &[0.0, 0.04, 0.08, 0.12]);
    let mask = math::drive_mask(ms);
    let g = family_inverse(&fam, &[1.0; 4], &mask, 1e-3).expect("a family inverse");
    equivalent_model(&g, &ms.plant.frf, &mask).expect("an equivalent model")
}

fn plants_from_env() -> Vec<ShockPlant> {
    match std::env::var("R3_PLANT").as_deref() {
        Ok("moderate") => vec![ShockPlant::Jointed4x4Moderate],
        Ok("severe") => vec![ShockPlant::Jointed4x4Severe],
        Ok("linear") => vec![ShockPlant::Linear4x4],
        _ => vec![
            ShockPlant::Linear4x4,
            ShockPlant::Jointed4x4Moderate,
            ShockPlant::Jointed4x4Severe,
        ],
    }
}

/// MS-R3-05: the family model's climbs (drops 8 and 12 %, three and four
/// members) and the warp follower, on the engine; the station seed from
/// `R3_SEED` (11 when unset).
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r3_study_family_and_warp() {
    type Make = fn(&math::Setup) -> MatrixStack;
    let rows: Vec<(&str, Make, bool)> = vec![
        ("frozen", identity_model, false),
        ("family 8 %", family8, false),
        ("family 12 %", family12, false),
        ("family 12 % in four", family12x4, false),
        ("warp", identity_model, true),
        ("warp on family 8 %", family8, true),
    ];
    for plant in plants_from_env() {
        run_header(&format!(
            "{}, twist SRS, station seed {}",
            plant.name(),
            std::env::var("R3_SEED").unwrap_or_else(|_| "11".into())
        ));
        for (name, make, warp) in &rows {
            for (lname, lad, per) in [
                (
                    "default ladder, 6 a level",
                    LadderPlan::default().levels_db,
                    6u32,
                ),
                ("from -40 by 3, 6 a level", ladder(-40.0, 3.0), 6),
            ] {
                let law = Arc::new(ModelShape {
                    make: *make,
                    clamp_db: None,
                    warp: *warp,
                    adaptive: None,
                });
                print_run(
                    &format!("{name}, {lname}, aborts off"),
                    &engine_run(plant, law, lad, per, false),
                );
            }
        }
    }
}

/// MS-R3-06: the research plant through the research ladder (-12, -9, -6,
/// -3, then ten pulses at full level; no pass logic), frozen and family.
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r3_study_research_plant() {
    use math::reference_plant::LADDER_DB;
    use math::research_plant::run_research_ladder;
    let b = research_bench();
    println!("\nresearch 4 x 4 at 8 %, mixed polarity, H1 identification, no noise\n");
    println!("| model | p1 over | worst first over | worst absolute below full | p5 over (first full) | p7 worst (third full) | p14 worst |");
    println!("|---|---|---|---|---|---|---|");
    let mut models: Vec<(String, MatrixStack)> = vec![("frozen".into(), b.ms.plant.frf.clone())];
    for d in [0.04, 0.08, 0.12] {
        models.push((format!("family {:.0} %", 100.0 * d), family_model(&b, d)));
    }
    for (name, m) in models {
        let mut law = b.law(&m);
        let mut resp =
            |d: &[Vec<f64>], s: f64| -> Result<Vec<Vec<f64>>, String> { Ok((b.respond)(d, s)) };
        let rows = run_research_ladder(
            &mut law, &b.ms, &mut resp, &LADDER_DB, 0.0, 20_261_111, None,
        )
        .expect("a run");
        let mut first = f64::NEG_INFINITY;
        let mut last = f64::NAN;
        for r in &rows {
            if r.level_db != last {
                first = first.max(r.over_db);
                last = r.level_db;
            }
        }
        let below = rows
            .iter()
            .filter(|r| r.level_db < 0.0)
            .map(|r| r.level_db + r.over_db)
            .fold(f64::NEG_INFINITY, f64::max);
        println!(
            "| {name} | {:+.2} | {:+.2} | {:+.2} | {:+.2} | {:.2} | {:.2} |",
            rows[0].over_db, first, below, rows[4].over_db, rows[6].worst_db, rows[13].worst_db
        );
    }
}

/// MS-R3-07: the probe-picked start. Two probes (-40 and -36 dB) through the
/// direct harness give the mismatch of the measured response against the
/// identification's prediction; `probe_start_level` (threshold 0.15, ceiling
/// -12 dB) picks the start; the first pulse there (direct), then the frozen
/// climb from it in 3 dB steps through the engine (the two probes are added
/// to its pulse count).
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r3_study_probe_start() {
    use math::first_pulse::probe_start_level;
    println!("\n| plant | mismatch -40 | mismatch -36 | start | p1 there (direct) | engine: p1 | worst first over | worst absolute below full | f1 | f3 err | n + 2 | end |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|");
    let mut benches: Vec<(Option<ShockPlant>, Bench)> = jointed_benches()
        .into_iter()
        .map(|(p, b)| (Some(p), b))
        .collect();
    benches.push((None, research_bench()));
    for (plant, b) in benches {
        let h = b.ms.plant.frf.clone();
        let mism = |l: f64| {
            let d = b.seed(&h, l);
            rel(&b.play(&d, l), &predict(&h, &d))
        };
        let (e1, e2) = (mism(-40.0), mism(-36.0));
        let start = probe_start_level([(-40.0, e1), (-36.0, e2)], 0.15, -12.0);
        let d = b.seed(&h, start);
        let (p1, _) = b.judge(&b.play(&d, start), start);
        let engine = plant.map(|p| {
            let law = Arc::new(ModelShape {
                make: identity_model,
                clamp_db: None,
                warp: false,
                adaptive: None,
            });
            engine_run(p, law, ladder(start, 3.0), 6, false)
        });
        let cols = match engine {
            Some(r) => format!(
                "{:+.2} | {:+.2} | {:+.2} | {} | {} | {} | {}",
                r.p1,
                r.worst_first_over,
                r.worst_abs_below,
                opt(r.f1),
                opt(r.f3),
                r.n + 2,
                r.end
            ),
            None => "- | - | - | - | - | - | -".into(),
        };
        println!(
            "| {} | {e1:.3} | {e2:.3} | {start:+.0} | {p1:+.2} | {cols} |",
            b.name
        );
    }
}

/// MS-R3-08: the family provider that follows the evidence (one-sided across
/// levels), beside the frozen family and the frozen identification.
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r3_study_adaptive_family() {
    const D8: &[f64] = &[0.0, 0.04, 0.08];
    const D12: &[f64] = &[0.0, 0.04, 0.08, 0.12];
    const D16: &[f64] = &[0.0, 0.04, 0.08, 0.12, 0.16];
    let rows: Vec<(&str, Option<(&'static [f64], bool)>)> = vec![
        ("adaptive to 8 %, within level", Some((D8, true))),
        ("adaptive to 12 %, within level", Some((D12, true))),
        ("adaptive to 12 %, steps only", Some((D12, false))),
        ("adaptive to 16 %, within level", Some((D16, true))),
    ];
    for plant in plants_from_env() {
        run_header(&format!("{}, twist SRS", plant.name()));
        for (name, a) in &rows {
            for (lname, lad, per) in [
                (
                    "default ladder, 3 a level",
                    LadderPlan::default().levels_db,
                    3u32,
                ),
                (
                    "default ladder, 6 a level",
                    LadderPlan::default().levels_db,
                    6,
                ),
                ("from -24 by 3, 6 a level", ladder(-24.0, 3.0), 6),
            ] {
                let law = Arc::new(ModelShape {
                    make: identity_model,
                    clamp_db: None,
                    warp: false,
                    adaptive: *a,
                });
                print_run(
                    &format!("{name}, {lname}, aborts off"),
                    &engine_run(plant, law, lad, per, false),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A second nonlinear plant of a different mechanism.
// ---------------------------------------------------------------------------

/// The severe plant's modes with an instantaneous softening spring in place
/// of the envelope law: mode `m` obeys
/// `q'' + 2 z w q' + w^2 (1 - kd min(1, |q| / q_ref)) q = u_m`, so its
/// stiffness falls with the displacement inside every cycle (it makes
/// harmonics; the envelope law does not), by up to `kd` (a frequency drop of
/// about `kd / 2`). No output delays, no noise. Fourth-order Runge-Kutta at the
/// sample rate with the drive linear inside a step.
fn softening_respond(jp: &JointedPlant, kd: f64, drives: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = drives[0].len();
    let dt = 1.0 / FS;
    let nd = jp.n_drives;
    let nc = jp.n_inputs;
    let mut out = vec![vec![0.0; n]; nc];
    let modes: Vec<(f64, f64, &[f64], &[f64], f64)> = jp
        .modes
        .iter()
        .map(|m| {
            (
                std::f64::consts::TAU * m.f_hz,
                m.zeta,
                m.input_shape.as_slice(),
                m.output_shape.as_slice(),
                m.reference_amplitude,
            )
        })
        .collect();
    let mut st: Vec<(f64, f64)> = vec![(0.0, 0.0); modes.len()];
    let force = |m: &(f64, f64, &[f64], &[f64], f64), t: usize, frac: f64| -> f64 {
        let mut u = 0.0;
        for j in 0..nd {
            let a = drives[j][t];
            let b = if t + 1 < n { drives[j][t + 1] } else { a };
            u += m.2[j] * (a + (b - a) * frac);
        }
        u
    };
    let acc = |m: &(f64, f64, &[f64], &[f64], f64), q: f64, v: f64, u: f64| -> f64 {
        let k = m.0 * m.0 * (1.0 - kd * (q.abs() / m.4).min(1.0));
        u - 2.0 * m.1 * m.0 * v - k * q
    };
    for t in 0..n {
        for (i, m) in modes.iter().enumerate() {
            let (q, v) = st[i];
            let a_now = acc(m, q, v, force(m, t, 0.0));
            for (r, row) in out.iter_mut().enumerate() {
                row[t] += m.3[r] * a_now;
            }
            // Advance to t + 1.
            let u0 = force(m, t, 0.0);
            let uh = force(m, t, 0.5);
            let u1 = force(m, t, 1.0);
            let k1 = (v, acc(m, q, v, u0));
            let k2 = (
                v + 0.5 * dt * k1.1,
                acc(m, q + 0.5 * dt * k1.0, v + 0.5 * dt * k1.1, uh),
            );
            let k3 = (
                v + 0.5 * dt * k2.1,
                acc(m, q + 0.5 * dt * k2.0, v + 0.5 * dt * k2.1, uh),
            );
            let k4 = (v + dt * k3.1, acc(m, q + dt * k3.0, v + dt * k3.1, u1));
            st[i] = (
                q + dt / 6.0 * (k1.0 + 2.0 * k2.0 + 2.0 * k3.0 + k4.0),
                v + dt / 6.0 * (k1.1 + 2.0 * k2.1 + 2.0 * k3.1 + k4.1),
            );
        }
        for (r, row) in out.iter_mut().enumerate() {
            let mut s = 0.0;
            for j in 0..nd {
                let rj = if jp.residual.is_empty() {
                    0.0
                } else {
                    jp.residual[r * nd + j]
                };
                s += (rj + jp.feedthrough) * drives[j][t];
            }
            row[t] += s;
        }
    }
    out
}

/// MS-R3-09: the softening-spring plant (a different mechanism) through fixed
/// ladders (no pass logic; the research ladder's form), identified by H1
/// random at the engine's rule (20 dB under the reference's rms): frozen and
/// family models, from -12 and from -40 by 3, each with eight full-level
/// pulses.
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r3_study_second_mechanism() {
    use math::research_plant::run_research_ladder;
    let base = jointed_bench(ShockPlant::Jointed4x4Severe, srs_twist(), LEN_SRS);
    let jp = ShockPlant::Jointed4x4Severe.plant();
    for kd in [0.16, 0.32] {
        let jp2 = jp.clone();
        let mut b = Bench {
            name: format!("softening spring, stiffness drop {:.0} %", 100.0 * kd),
            ms: base.ms.clone(),
            respond: Box::new(move |d, _| softening_respond(&jp2, kd, d)),
            jointed: None,
            warp: warp_for(&base.ms),
        };
        let (h, v) = random_id(&b, 20.0);
        b.ms.plant.frf = h;
        b.warp = warp_for(&b.ms);
        println!("\n{} (identified at {v:.3} V rms)\n", b.name);
        // The first pulse by level, frozen.
        let firsts: Vec<String> = [-40.0, -32.0, -24.0, -18.0, -12.0, 0.0]
            .iter()
            .map(|&l| {
                let (o, _) = b.judge(&b.play(&b.seed(&b.ms.plant.frf, l), l), l);
                format!("{l:+.0}: {o:+.2}")
            })
            .collect();
        println!("frozen first pulse by level: {}\n", firsts.join(", "));
        println!("| model, ladder | p1 | worst first over | worst absolute below full | first full over | third full worst | last worst | n |");
        println!("|---|---|---|---|---|---|---|---|");
        let mut models: Vec<(String, MatrixStack)> =
            vec![("frozen".into(), b.ms.plant.frf.clone())];
        for d in [0.04, 0.08, 0.12] {
            models.push((format!("family {:.0} %", 100.0 * d), family_model(&b, d)));
        }
        let full = |v: Vec<f64>| -> Vec<f64> { v.into_iter().chain([0.0; 7]).collect() };
        for (name, m) in &models {
            for (lname, lad) in [
                ("from -12", full(LadderPlan::default().levels_db)),
                ("from -40 by 3", full(ladder(-40.0, 3.0))),
            ] {
                let mut law = b.law(m);
                let mut resp = |d: &[Vec<f64>], s: f64| -> Result<Vec<Vec<f64>>, String> {
                    Ok((b.respond)(d, s))
                };
                let rows = run_research_ladder(&mut law, &b.ms, &mut resp, &lad, 0.0, 1, None)
                    .expect("a run");
                let mut first = f64::NEG_INFINITY;
                let mut last = f64::NAN;
                for r in &rows {
                    if r.level_db != last {
                        first = first.max(r.over_db);
                        last = r.level_db;
                    }
                }
                let below = rows
                    .iter()
                    .filter(|r| r.level_db < 0.0)
                    .map(|r| r.level_db + r.over_db)
                    .fold(f64::NEG_INFINITY, f64::max);
                let fulls: Vec<_> = rows.iter().filter(|r| r.level_db == 0.0).collect();
                println!(
                    "| {name}, {lname} | {:+.2} | {first:+.2} | {below:+.2} | {:+.2} | {:.2} | {:.2} | {} |",
                    rows[0].over_db,
                    fulls[0].over_db,
                    fulls[2].worst_db,
                    rows.last().map(|r| r.worst_db).unwrap_or(f64::NAN),
                    rows.len()
                );
            }
        }
    }
}

/// MS-R3-10: can the frozen climb pass at full level on the severe plant with
/// a larger allowance of pulses a level?
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r3_study_long_climb() {
    let plant = ShockPlant::Jointed4x4Severe;
    run_header(&format!(
        "{}, twist SRS, the frozen climb with more pulses a level",
        plant.name()
    ));
    for (start, step) in [(-40.0, 3.0), (-40.0, 1.5), (-33.0, 3.0)] {
        for per in [12u32, 20] {
            let law = Arc::new(ModelShape {
                make: identity_model,
                clamp_db: None,
                warp: false,
                adaptive: None,
            });
            print_run(
                &format!("frozen from {start} by {step}, {per} a level, aborts off"),
                &engine_run(plant, law, ladder(start, step), per, false),
            );
        }
    }
}
