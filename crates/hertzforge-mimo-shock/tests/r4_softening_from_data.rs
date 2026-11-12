//! Research round 4: the family inverse's softening set from the test's own
//! pulses, and identification from low-level pulses. Studies only (ignored,
//! run on demand; each prints its table).
//!
//! Harnesses as round 3's (`r3_inside_the_pulse.rs`): the direct one takes
//! the engine's own identification and then plays drives through the plant's
//! noise-free simulation; the engine one is the whole run through the host
//! firer. The research plant and the softening spring run on the research
//! ladder runner (fixed ladders, no pass logic).
//!
//! Requirements R4-A to R4-G and the expectations were fixed in the research
//! record's experiment log before this file was written.

#![allow(clippy::needless_range_loop, clippy::type_complexity)]

mod support;

use hertzforge_hal_sim::jointed::{JointLaw, JointedPlant};
use hertzforge_hal_sim::plant::PlantModel;
use hertzforge_hal_sim::plants::ShockPlant;
use hertzforge_mimo_shock::builtin::{math_setup, InverseFactory};
use hertzforge_mimo_shock::{LadderPlan, MimoShockLawFactory};
use hertzforge_mimo_shock_law_math as math;
use math::first_pulse::{coded_frf, equivalent_model, family_inverse, stretched_family};
use math::provider::{FrozenModel, ModelProvider, ModelReport, ModelSource, Observation};
use math::reference_plant::FS_HZ as RFS;
use math::research_plant::{
    h1_from_records, identify_h1_model, research_setup_signed, run_research_ladder, LevelPlant,
};
use math::shape::{CarryGain, ShapeExtras, ShapeLaw, ShapeParams, ShapeSafety};
use math::softening::{fit_softening, Bands, FitOptions, Follow, SofteningFamily, SofteningPlan};
use math::spectrum::{apply, rfft, rows_irfft, rows_rfft};
use math::srs::{error_db, srs_rows};
use math::warp::{WarpModel, WarpOptions};
use math::{level_scale, LawSteps, MatrixStack, C64};
use std::sync::{Arc, Mutex};
use support::*;

// ---------------------------------------------------------------------------
// Plants.
// ---------------------------------------------------------------------------

/// A jointed plant run through a simulator station.
#[derive(Clone)]
struct Plant {
    name: String,
    jp: JointedPlant,
}

fn shock(p: ShockPlant) -> Plant {
    Plant {
        name: p.name().into(),
        jp: p.plant(),
    }
}

/// The non-saturating jointed plant (fixed in the experiment log before any
/// round 4 code): the severe plant's structure, drop 0.15 with exponent 0.5,
/// no damping rise, every reference amplitude 25 times the severe plant's.
fn nonsat() -> Plant {
    let mut jp = ShockPlant::Jointed4x4Severe.plant();
    for m in jp.modes.iter_mut() {
        m.reference_amplitude *= 25.0;
    }
    jp.joint = Some(JointLaw {
        delta: 0.15,
        damping_gain: 0.0,
        freq_exponent: 0.5,
        damping_exponent: 0.5,
        decay_cycles: 3.0,
    });
    Plant {
        name: "jointed-4x4-non-saturating".into(),
        jp,
    }
}

/// The held-out plant of the operator-free part (fixed in the experiment
/// log before it was run): the severe plant's structure, drop 0.10 with
/// exponent 2 (softening that sets in late), no damping rise, every reference
/// amplitude 15 times the severe plant's.
fn late() -> Plant {
    let mut jp = ShockPlant::Jointed4x4Severe.plant();
    for m in jp.modes.iter_mut() {
        m.reference_amplitude *= 15.0;
    }
    jp.joint = Some(JointLaw {
        delta: 0.10,
        damping_gain: 0.0,
        freq_exponent: 2.0,
        damping_exponent: 0.5,
        decay_cycles: 3.0,
    });
    Plant {
        name: "jointed-4x4-late-softening".into(),
        jp,
    }
}

fn engine_plants() -> Vec<Plant> {
    if std::env::var("R4_PLANT").as_deref() == Ok("late") {
        return vec![late()];
    }
    let all = vec![
        shock(ShockPlant::Linear4x4),
        shock(ShockPlant::Jointed4x4Moderate),
        shock(ShockPlant::Jointed4x4Severe),
        nonsat(),
    ];
    match std::env::var("R4_PLANT").as_deref() {
        Ok("linear") => vec![all[0].clone()],
        Ok("moderate") => vec![all[1].clone()],
        Ok("severe") => vec![all[2].clone()],
        Ok("nonsat") => vec![all[3].clone()],
        _ => all,
    }
}

/// Measurement noise relative to the full-level reference's rms, from
/// `R4_NOISE` (none beyond the station's own 1e-4 when unset).
fn noise_rel() -> f64 {
    std::env::var("R4_NOISE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0)
}

/// The twist SRS reference's rms (every engine plant shares it).
fn reference_rms() -> f64 {
    static RMS: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *RMS.get_or_init(|| {
        let b = jointed_bench_noise(&shock(ShockPlant::Linear4x4), 1e-4);
        math::spectrum::rms_all(&b.ms.reference)
    })
}

fn station_of(p: &Plant, seed: u64) -> Box<dyn hertzforge_hal::Backend + Send> {
    let n = noise_rel();
    let abs = if n > 0.0 {
        (n * reference_rms()).max(1e-4)
    } else {
        1e-4
    };
    station_noise(p, seed, abs)
}

fn station_noise(p: &Plant, seed: u64, noise: f64) -> Box<dyn hertzforge_hal::Backend + Send> {
    Box::new(
        hertzforge_hal_sim::SimConfig::new()
            .sample_rate_hz(FS)
            .noise_level(noise)
            .shared_clock(true)
            .seed(seed)
            .plant(PlantModel::Jointed(p.jp.clone()))
            .build(),
    )
}

/// The first mode's peak drop for drives `d` (the joint law at its peak envelope).
fn first_mode_drop(jp: &JointedPlant, d: &[Vec<f64>]) -> f64 {
    let Some(law) = jp.joint else { return 0.0 };
    let sim = jp.simulate(FS, d).expect("simulated");
    let r = sim.peak_ratio(jp)[0];
    1.0 - law.frequency_factor(r)
}

// ---------------------------------------------------------------------------
// The direct harness.
// ---------------------------------------------------------------------------

type Respond = Box<dyn Fn(&[Vec<f64>], f64) -> Vec<Vec<f64>>>;

struct Bench {
    name: String,
    ms: math::Setup,
    respond: Respond,
    jointed: Option<JointedPlant>,
}

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

fn warp_on(ms: &math::Setup, model: &MatrixStack) -> WarpModel {
    WarpModel::new(
        model.clone(),
        math::drive_mask(ms),
        ms.fs_hz,
        warp_options(ms),
    )
    .expect("a warp")
}

fn jointed_bench(p: &Plant) -> Bench {
    jointed_bench_noise(p, 1e-4)
}

fn jointed_bench_noise(p: &Plant, noise: f64) -> Bench {
    let cfg = config(4, 0, 4, LEN_SRS);
    let mut e = hertzforge_mimo_shock::MimoShockEngine::new(
        cfg,
        Arc::new(hertzforge_mimo_shock::builtin::ShapeFactory::default()),
    )
    .expect("a configuration")
    .with_target_spec(srs_twist())
    .with_clock(Arc::new(ManualClock::default()))
    .with_pacing(false);
    e.arm(
        station_noise(p, 11, noise),
        table(4, 0, 4),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .expect("armed");
    e.identify().expect("identified");
    let ms = math_setup(e.setup().expect("a setup"));
    let jp = p.jp.clone();
    let jp2 = jp.clone();
    Bench {
        name: p.name.clone(),
        ms,
        respond: Box::new(move |d, _| jp2.simulate(FS, d).expect("simulated").outputs),
        jointed: Some(jp),
    }
}

fn research_bench() -> Bench {
    let p = LevelPlant::four_by_four_damped(0.08, 0.0, 0.02);
    let h1 = identify_h1_model(&p.modal_truth(0.1), 16, 0.05, 7).expect("identified");
    let pol: Vec<f64> = (0..p.n_c)
        .map(|c| if c % 2 == 1 { -1.0 } else { 1.0 })
        .collect();
    let ms = research_setup_signed(h1, &pol);
    Bench {
        name: "research-4x4-8%".into(),
        ms,
        respond: Box::new(move |d, s| p.respond(d, s, RFS).expect("responded")),
        jointed: None,
    }
}

/// Round 3's softening spring (MS-R3-09), identified by H1 random 20 dB under
/// the reference's rms.
fn spring_bench(kd: f64) -> Bench {
    let base = jointed_bench(&shock(ShockPlant::Jointed4x4Severe));
    let jp = ShockPlant::Jointed4x4Severe.plant();
    let mut b = Bench {
        name: format!("softening spring {:.0} %", 100.0 * kd),
        ms: base.ms.clone(),
        respond: Box::new(move |d, _| softening_respond(&jp, kd, d)),
        jointed: None,
    };
    let h = random_id(&b, 20.0);
    b.ms.plant.frf = h;
    b
}

impl Bench {
    fn mask(&self) -> Vec<bool> {
        math::drive_mask(&self.ms)
    }

    fn seed(&self, model: &MatrixStack, level_db: f64) -> Vec<Vec<f64>> {
        frozen_law(&self.ms, model)
            .seed(level_db)
            .expect("seeded")
            .drives
    }

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

    fn play(&self, d: &[Vec<f64>], level_db: f64) -> Vec<Vec<f64>> {
        (self.respond)(d, level_scale(level_db))
    }
}

fn frozen_law(ms: &math::Setup, model: &MatrixStack) -> ShapeLaw {
    ShapeLaw::with_options(
        ms.clone(),
        ShapeParams::default(),
        ShapeExtras {
            provider: Some(Box::new(FrozenModel::new(
                model.clone(),
                math::drive_mask(ms),
            ))),
            safety: ShapeSafety::off(),
            robust_first: None,
        },
    )
    .expect("a law")
}

fn rms_rows(r: &[Vec<f64>]) -> Vec<f64> {
    r.iter()
        .map(|x| (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt())
        .collect()
}

/// H1 from band-limited random drives with the control nearest its mark
/// `below_db` under its reference's rms (round 3's `random_id`).
fn random_id(b: &Bench, below_db: f64) -> MatrixStack {
    let n = b.ms.record_len;
    let len = n * 32;
    let nd = b.ms.n_drives;
    let mut rng = math::reference_plant::Rng::new(99);
    let unit: Vec<Vec<f64>> = (0..nd)
        .map(|_| {
            let x: Vec<f64> = (0..len).map(|_| rng.normal()).collect();
            let mut s = rfft(&x).expect("a spectrum");
            let df = b.ms.fs_hz / len as f64;
            for (k, v) in s.iter_mut().enumerate() {
                if !(5.0..=2500.0).contains(&(k as f64 * df)) {
                    *v = C64::new(0.0, 0.0);
                }
            }
            math::spectrum::irfft(&s, len).expect("a record")
        })
        .collect();
    let y1 = b.play(&unit, 0.0);
    let ry = rms_rows(&y1);
    let rr = rms_rows(&b.ms.reference);
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
    h1_from_records(&drives, &y, n).expect("an H1")
}

/// Round 3's softening spring (see `r3_inside_the_pulse.rs`, MS-R3-09).
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

fn hadamard4() -> Vec<Vec<f64>> {
    vec![
        vec![1.0, 1.0, 1.0, 1.0],
        vec![1.0, -1.0, 1.0, -1.0],
        vec![1.0, 1.0, -1.0, -1.0],
        vec![1.0, -1.0, -1.0, 1.0],
    ]
}

/// Identification from four coded copies of the frozen first drive at
/// `level_db` (round 3's `coded_frf`, floor 1e-2, the random identification
/// as the prior below it). Returns the model and the worst absolute level of
/// any of the four shots (its SRS over the full-level target, dB).
fn pulse_id(b: &Bench, level_db: f64) -> (MatrixStack, f64) {
    pulse_id_with(b, level_db, &hadamard4())
}

/// One drive at a time (the identity code): four shots, each drive alone.
fn one_at_a_time() -> Vec<Vec<f64>> {
    (0..4)
        .map(|k| (0..4).map(|j| if j == k { 1.0 } else { 0.0 }).collect())
        .collect()
}

fn pulse_id_with(b: &Bench, level_db: f64, codes: &[Vec<f64>]) -> (MatrixStack, f64) {
    let d = b.seed(&b.ms.plant.frf, level_db);
    let codes = codes.to_vec();
    let mut worst_abs = f64::NEG_INFINITY;
    let shots: Vec<Vec<Vec<C64>>> = codes
        .iter()
        .map(|c| {
            let dc: Vec<Vec<f64>> = d
                .iter()
                .zip(c)
                .map(|(r, s)| r.iter().map(|v| v * s).collect())
                .collect();
            let y = b.play(&dc, level_db);
            let (o, _) = b.judge(&y, level_db);
            worst_abs = worst_abs.max(level_db + o);
            rows_rfft(&y, 1.0).expect("spectra")
        })
        .collect();
    let ds = rows_rfft(&d, 1.0).expect("spectra");
    (
        coded_frf(&ds, &shots, &codes, &b.ms.plant.frf, 1e-2).expect("a coded model"),
        worst_abs,
    )
}

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

/// The family model over `0, drop/2, drop` (round 3's).
fn family_of(ms: &math::Setup, base: &MatrixStack, drop: f64) -> MatrixStack {
    if drop <= 0.0 {
        return base.clone();
    }
    let w = warp_on(ms, base);
    let fam = stretched_family(&w, &[0.0, 0.5 * drop, drop]);
    let mask = math::drive_mask(ms);
    let g = family_inverse(&fam, &[1.0, 1.0, 1.0], &mask, 1e-3).expect("a family inverse");
    equivalent_model(&g, base, &mask).expect("an equivalent model")
}

// ---------------------------------------------------------------------------
// The methods, as a law built on the setup.
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum Base {
    /// The engine's identification.
    Ident,
    /// A given model (the pulse identification).
    Given(Arc<MatrixStack>),
}

#[derive(Clone)]
struct Method {
    base: Base,
    /// A hand-set family over the base, or none.
    family: Option<f64>,
    /// The softening provider, and an assumed drop to start it at.
    softening: Option<(SofteningPlan, Option<f64>)>,
    params: ShapeParams,
    carry: CarryGain,
    log: Arc<Mutex<Vec<String>>>,
}

impl Method {
    fn frozen() -> Self {
        Method {
            base: Base::Ident,
            family: None,
            softening: None,
            params: ShapeParams::default(),
            carry: CarryGain::Keep,
            log: Arc::default(),
        }
    }
    fn family(d: f64) -> Self {
        Method {
            family: Some(d),
            ..Method::frozen()
        }
    }
    fn soft(plan: SofteningPlan) -> Self {
        Method {
            softening: Some((plan, None)),
            ..Method::frozen()
        }
    }
    fn on(mut self, m: &Arc<MatrixStack>) -> Self {
        self.base = Base::Given(m.clone());
        self
    }

    fn law(&self, ms: &math::Setup) -> Result<ShapeLaw, String> {
        let base = match &self.base {
            Base::Ident => ms.plant.frf.clone(),
            Base::Given(m) => (**m).clone(),
        };
        let mask = math::drive_mask(ms);
        let provider: Box<dyn ModelProvider> = match &self.softening {
            Some((plan, start)) => {
                let mut f = SofteningFamily::new(
                    warp_on(ms, &base),
                    base.clone(),
                    mask,
                    rows_rfft(&ms.reference, 1.0)?,
                    plan.clone(),
                )?;
                if let Some(d) = start {
                    f.start_at(*d)?;
                }
                Box::new(Logged {
                    inner: f,
                    log: self.log.clone(),
                    seen: 0,
                })
            }
            None => {
                let m = match self.family {
                    Some(d) => family_of(ms, &base, d),
                    None => base,
                };
                Box::new(FrozenModel::new(m, mask))
            }
        };
        ShapeLaw::with_options(
            ms.clone(),
            self.params.clone(),
            ShapeExtras {
                provider: Some(provider),
                safety: ShapeSafety {
                    carry: self.carry,
                    ..ShapeSafety::off()
                },
                robust_first: None,
            },
        )
    }
}

/// The softening provider, writing every fit and every change to a log.
struct Logged {
    inner: SofteningFamily,
    log: Arc<Mutex<Vec<String>>>,
    seen: usize,
}

fn pct(v: &[f64]) -> String {
    v.iter()
        .map(|x| format!("{:.1}", 100.0 * x))
        .collect::<Vec<_>>()
        .join(" ")
}

impl ModelProvider for Logged {
    fn current(&self) -> &MatrixStack {
        self.inner.current()
    }
    fn observe(&mut self, obs: &Observation<'_>) -> Result<(), String> {
        self.inner.observe(obs)?;
        if let Some(l) = self.inner.levels().last() {
            self.log.lock().expect("log").push(format!(
                "fit at {:+.1}: drops % [{}] amplitude [{}]",
                l.level_db,
                pct(&l.fit.drops),
                l.fit
                    .amplitude
                    .iter()
                    .map(|a| format!("{a:.3}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        Ok(())
    }
    fn model_for(&mut self, next_level_db: f64) -> Result<ModelSource, String> {
        let r = self.inner.model_for(next_level_db)?;
        let ch = self.inner.changes();
        for (l, d) in &ch[self.seen..] {
            self.log
                .lock()
                .expect("log")
                .push(format!("assumed for {l:+.1}: % [{}]", pct(d)));
        }
        self.seen = ch.len();
        Ok(r)
    }
    fn report(&self) -> ModelReport {
        self.inner.report()
    }
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

type Fault = hertzforge_shock_common::law_guard::LawFault;

impl hertzforge_mimo_shock::MimoShockLaw for Adapter {
    fn name(&self) -> &str {
        "shape-softening"
    }
    fn seed(&mut self, level_db: f64) -> Result<hertzforge_mimo_shock::PulseOutputs, Fault> {
        self.law.seed(level_db).map(outs).map_err(Fault::Failed)
    }
    fn iterate(
        &mut self,
        i: &hertzforge_mimo_shock::PulseInputs,
    ) -> Result<hertzforge_mimo_shock::PulseOutputs, Fault> {
        let rows = |f: &[f64], n: usize| f.chunks(n.max(1)).map(|c| c.to_vec()).collect();
        let m = math::PulseInputs {
            pulse_index: i.pulse_index as usize - 1,
            level_db: i.level_db,
            next_level_db: i.next_level_db,
            drives: rows(&i.drives, self.len),
            responses: rows(&i.responses, self.len),
            srs: i.srs.as_ref().map(|s| rows(s, self.nf)),
        };
        self.law.iterate(&m).map(outs).map_err(Fault::Failed)
    }
}

impl MimoShockLawFactory for Method {
    fn build(
        &self,
        setup: &hertzforge_mimo_shock::Setup,
    ) -> Result<Box<dyn hertzforge_mimo_shock::MimoShockLaw + Send>, Fault> {
        let ms = math_setup(setup);
        let nf = ms.srs.as_ref().map(|t| t.natural_hz.len()).unwrap_or(0);
        let law = self.law(&ms).map_err(Fault::Failed)?;
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

// ---------------------------------------------------------------------------
// Runs.
// ---------------------------------------------------------------------------

/// One run's numbers. `first` is the first pulse at or above the first ladder
/// level; `probe_over` the worst over-test of the pulses below it.
#[derive(Debug, Clone)]
struct Run {
    probe_over: Option<f64>,
    first: f64,
    worst_first_over: f64,
    worst_abs_below: f64,
    f1: Option<f64>,
    f3: Option<f64>,
    last: Option<f64>,
    n: usize,
    end: String,
}

fn summarize(rows: &[(f64, f64, f64)], ladder_start: f64, end: String) -> Run {
    // rows: (level, over, worst |error|)
    let probes: Vec<_> = rows.iter().filter(|r| r.0 < ladder_start - 1e-9).collect();
    let ladder: Vec<_> = rows.iter().filter(|r| r.0 >= ladder_start - 1e-9).collect();
    let mut worst_first = f64::NEG_INFINITY;
    let mut last_level = f64::NAN;
    for r in &ladder {
        if r.0 != last_level {
            worst_first = worst_first.max(r.1);
            last_level = r.0;
        }
    }
    let full: Vec<_> = ladder.iter().filter(|r| r.0 == 0.0).collect();
    Run {
        probe_over: probes
            .iter()
            .map(|r| r.1)
            .fold(None, |a: Option<f64>, v| Some(a.map_or(v, |x| x.max(v)))),
        first: ladder.first().map(|r| r.1).unwrap_or(f64::NAN),
        worst_first_over: worst_first,
        worst_abs_below: rows
            .iter()
            .filter(|r| r.0 < 0.0)
            .map(|r| r.0 + r.1)
            .fold(f64::NEG_INFINITY, f64::max),
        f1: full.first().map(|r| r.1),
        f3: full.get(2).map(|r| r.2),
        last: full.last().map(|r| r.2),
        n: rows.len(),
        end,
    }
}

fn seed_env() -> u64 {
    std::env::var("R4_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(11)
}

fn engine_run(
    p: &Plant,
    law: Arc<dyn MimoShockLawFactory>,
    ladder: Vec<f64>,
    per_level: u32,
    ladder_start: f64,
) -> Run {
    let mut cfg = config(4, 0, 4, LEN_SRS);
    cfg.ladder = LadderPlan {
        levels_db: ladder,
        max_pulses_per_level: per_level,
        ..LadderPlan::default()
    };
    cfg.guards.scaled_aborts = false;
    cfg.guards.law.max_pulses = cfg.guards.law.max_pulses.max(200u64);
    let mut e = hertzforge_mimo_shock::MimoShockEngine::new(cfg, law)
        .expect("a good configuration")
        .with_target_spec(srs_twist())
        .with_clock(Arc::new(ManualClock::default()))
        .with_pacing(false);
    e.arm(
        station_of(p, seed_env()),
        table(4, 0, 4),
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
    let rows: Vec<(f64, f64, f64)> = e
        .pulses()
        .iter()
        .map(|q| {
            (
                q.level_db,
                q.over_test_db,
                q.controls
                    .iter()
                    .fold(0.0f64, |a, c| a.max(c.verdict.error_db.abs())),
            )
        })
        .collect();
    let end = e
        .end()
        .map(|x| format!("{:?} {}", x.outcome, x.reason))
        .unwrap_or_else(|| "none".into());
    summarize(&rows, ladder_start, end)
}

fn runner_run(b: &Bench, m: &Method, ladder: &[f64], ladder_start: f64) -> Run {
    let mut law = m.law(&b.ms).expect("a law");
    let mut resp =
        |d: &[Vec<f64>], s: f64| -> Result<Vec<Vec<f64>>, String> { Ok((b.respond)(d, s)) };
    match run_research_ladder(
        &mut law,
        &b.ms,
        &mut resp,
        ladder,
        noise_rel(),
        20_261_111,
        None,
    ) {
        Ok(rows) => {
            let r: Vec<(f64, f64, f64)> = rows
                .iter()
                .map(|r| (r.level_db, r.over_db, r.worst_db))
                .collect();
            summarize(&r, ladder_start, "fixed ladder".into())
        }
        Err(e) => summarize(&[], ladder_start, format!("law error: {e}")),
    }
}

fn opt(o: Option<f64>) -> String {
    o.map(|v| format!("{v:+6.2}"))
        .unwrap_or_else(|| "    - ".into())
}

fn header(title: &str) {
    println!("\n{title}\n");
    println!("| run | probes over | first | worst first over | worst abs below full | f1 | f3 err | last err | n | end |");
    println!("|---|---|---|---|---|---|---|---|---|---|");
}

fn print_run(label: &str, extra_pulses: usize, r: &Run) {
    println!(
        "| {label} | {} | {:+6.2} | {:+6.2} | {:+6.2} | {} | {} | {} | {} | {} |",
        opt(r.probe_over),
        r.first,
        r.worst_first_over,
        r.worst_abs_below,
        opt(r.f1),
        opt(r.f3),
        opt(r.last),
        r.n + extra_pulses,
        r.end
    );
}

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

const PROBES: [f64; 2] = [-40.0, -34.0];

fn with_probes(l: &[f64]) -> Vec<f64> {
    PROBES.iter().copied().chain(l.iter().copied()).collect()
}

/// The method settings of this round, by name (fixed in the experiment log
/// before the held-out plants run).
fn plan(follow: Follow) -> SofteningPlan {
    SofteningPlan {
        follow,
        ladder_start_db: Some(-12.0),
        ..SofteningPlan::default()
    }
}

const WIDEN: Follow = Follow::Levels {
    narrow: 1.0,
    within_level: false,
};
const SLOW: Follow = Follow::Levels {
    narrow: 0.7,
    within_level: true,
};

fn flush(m: &Method) {
    let mut l = m.log.lock().expect("log");
    if std::env::var("R4_VERBOSE").is_ok() {
        for s in l.iter() {
            println!("    {s}");
        }
    }
    l.clear();
}

// ---------------------------------------------------------------------------
// Studies.
// ---------------------------------------------------------------------------

/// MS-R4-01: the fitted drop by level (direct harness, the frozen first
/// pulse at each level): every band's fit, the one-band fit, the first-mode
/// band's amplitude and the first mode's true peak drop.
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r4_study_fit_by_level() {
    let mut benches: Vec<Bench> = engine_plants().iter().map(jointed_bench).collect();
    if std::env::var("R4_PLANT").is_err() {
        benches.push(research_bench());
        benches.push(spring_bench(0.16));
    }
    for b in &benches {
        let h = b.ms.plant.frf.clone();
        let w = warp_on(&b.ms, &h);
        let mask = b.mask();
        let bands = Bands::octaves(&w, &mask, 1.0).expect("bands");
        let one = Bands::single(&w, &mask).expect("one band");
        let reference = rows_rfft(&b.ms.reference, 1.0).expect("spectra");
        let knots = w.knot_hz();
        println!(
            "\n{}: bands from {:.0} Hz, one octave each ({} bands)\n",
            b.name,
            knots[0],
            bands.len()
        );
        println!("| level | frozen over | true peak drop mode 1 % | fit by band % | raw by band % | one band % | amplitude band 1 | residual band 1 at 0 / fit |");
        println!("|---|---|---|---|---|---|---|---|");
        for &l in &[-40.0, -34.0, -28.0, -24.0, -18.0, -12.0, -6.0, 0.0] {
            let d = b.seed(&h, l);
            let y = b.play(&d, l);
            let (over, _) = b.judge(&y, l);
            let s = level_scale(l);
            let p = math::softening::PulseSpectra {
                drives: rows_rfft(&d, s).expect("spectra"),
                responses: rows_rfft(&y, s).expect("spectra"),
                scale: s,
            };
            let t0 = std::time::Instant::now();
            let fit = fit_softening(
                &w,
                &bands,
                std::slice::from_ref(&p),
                &reference,
                &FitOptions::default(),
            )
            .expect("a fit");
            let el = t0.elapsed().as_secs_f64();
            let f1 = fit_softening(
                &w,
                &one,
                std::slice::from_ref(&p),
                &reference,
                &FitOptions::default(),
            )
            .expect("a fit");
            let truth = match &b.jointed {
                Some(jp) => format!("{:.2}", 100.0 * first_mode_drop(jp, &d)),
                None => "-".into(),
            };
            println!(
                "| {l:+.0} | {over:+.2} | {truth} | {} | {} | {:.2} | {:.4} | {:.3} / {:.3} ({el:.2} s) |",
                pct(&fit.drops),
                pct(&fit.raw_drops),
                100.0 * f1.drops[0],
                fit.amplitude[1],
                fit.residual_at_zero[1],
                fit.residual[1]
            );
        }
        if let Some(jp) = &b.jointed {
            if jp.joint.is_some() {
                let d = b.seed(&family_of(&b.ms, &h, 0.08), 0.0);
                let sim = jp.simulate(FS, &d).expect("simulated");
                println!(
                    "full level (family 8 % drive): peak envelope ratio by mode {:?}",
                    sim.peak_ratio(jp)
                        .iter()
                        .map(|r| format!("{r:.2}"))
                        .collect::<Vec<_>>()
                );
            }
        }
    }
}

/// MS-R4-02 to -04 on the engine's plants: every method beside the three
/// baselines. `R4_PLANT` picks a plant, `R4_SEED` the station seed.
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r4_study_methods() {
    let def = LadderPlan::default().levels_db;
    for p in engine_plants() {
        let t0 = std::time::Instant::now();
        header(&format!(
            "{}, twist SRS, station seed {}, 6 a level",
            p.name,
            seed_env()
        ));
        let b = jointed_bench(&p);
        let (pid, pid_abs) = pulse_id(&b, -18.0);
        let pid = Arc::new(pid);
        let rows: Vec<(String, Method, Vec<f64>, usize)> = vec![
            (
                "frozen, default ladder".into(),
                Method::frozen(),
                def.clone(),
                0,
            ),
            (
                "frozen, -40 by 3".into(),
                Method::frozen(),
                ladder(-40.0, 3.0),
                0,
            ),
            (
                "family 8 % (hand-set)".into(),
                Method::family(0.08),
                def.clone(),
                0,
            ),
            (
                "softening from probes".into(),
                Method::soft(plan(Follow::Probes)),
                with_probes(&def),
                0,
            ),
            (
                "softening followed, widen only".into(),
                Method::soft(plan(WIDEN)),
                with_probes(&def),
                0,
            ),
            (
                "softening followed, narrow 0.7 within level".into(),
                Method::soft(plan(SLOW)),
                with_probes(&def),
                0,
            ),
            (
                format!("pulse id at -18 (shots worst abs {pid_abs:+.1}), frozen"),
                Method::frozen().on(&pid),
                def.clone(),
                4,
            ),
            (
                "pulse id at -18, softening followed, widen only".into(),
                Method::soft(SofteningPlan {
                    probe_levels: 0,
                    ..plan(WIDEN)
                })
                .on(&pid),
                def.clone(),
                4,
            ),
            (
                "pulse id at -18, softening followed, narrow 0.7 within level".into(),
                Method::soft(SofteningPlan {
                    probe_levels: 0,
                    ..plan(SLOW)
                })
                .on(&pid),
                def.clone(),
                4,
            ),
        ];
        for (name, m, lad, extra) in rows {
            let r = engine_run(&p, Arc::new(m.clone()), lad, 6, -12.0);
            print_run(&name, extra, &r);
            flush(&m);
        }
        println!("({:.0} s)", t0.elapsed().as_secs_f64());
    }
}

/// MS-R4-05: the research plant and the softening springs on the research
/// ladder runner, the same methods.
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r4_study_runner_plants() {
    use math::reference_plant::LADDER_DB;
    let which = std::env::var("R4_RUNNER").unwrap_or_else(|_| "dev".into());
    let mut benches: Vec<(Bench, Vec<f64>)> = Vec::new();
    let spring_ladder: Vec<f64> = LadderPlan::default()
        .levels_db
        .into_iter()
        .chain([0.0; 7])
        .collect();
    if which != "heldout" {
        benches.push((research_bench(), LADDER_DB.to_vec()));
        benches.push((spring_bench(0.16), spring_ladder.clone()));
    }
    if which != "dev" {
        benches.push((spring_bench(0.32), spring_ladder.clone()));
    }
    for (b, lad) in &benches {
        header(&format!("{} (research ladder runner)", b.name));
        let (pid, pid_abs) = pulse_id(b, -18.0);
        let pid = Arc::new(pid);
        let rows: Vec<(String, Method, Vec<f64>, usize)> = vec![
            ("frozen, from -12".into(), Method::frozen(), lad.clone(), 0),
            (
                "frozen, -40 by 3".into(),
                Method::frozen(),
                ladder(-40.0, 3.0).into_iter().chain([0.0; 7]).collect(),
                0,
            ),
            (
                "family 8 % (hand-set)".into(),
                Method::family(0.08),
                lad.clone(),
                0,
            ),
            (
                "softening from probes".into(),
                Method::soft(plan(Follow::Probes)),
                with_probes(lad),
                0,
            ),
            (
                "softening followed, widen only".into(),
                Method::soft(plan(WIDEN)),
                with_probes(lad),
                0,
            ),
            (
                "softening followed, narrow 0.7 within level".into(),
                Method::soft(plan(SLOW)),
                with_probes(lad),
                0,
            ),
            (
                format!("pulse id at -18 (shots worst abs {pid_abs:+.1}), frozen"),
                Method::frozen().on(&pid),
                lad.clone(),
                4,
            ),
            (
                "pulse id at -18, softening followed, widen only".into(),
                Method::soft(SofteningPlan {
                    probe_levels: 0,
                    ..plan(WIDEN)
                })
                .on(&pid),
                lad.clone(),
                4,
            ),
            (
                "pulse id at -18, softening followed, narrow 0.7 within level".into(),
                Method::soft(SofteningPlan {
                    probe_levels: 0,
                    ..plan(SLOW)
                })
                .on(&pid),
                lad.clone(),
                4,
            ),
        ];
        for (name, m, l, extra) in rows {
            let r = runner_run(b, &m, &l, -12.0);
            print_run(&name, extra, &r);
            flush(&m);
        }
    }
}

/// MS-R4-06: the first pulse after a pulse identification by its level
/// (direct harness): the first ladder level at -12 dB, and 6 dB above the
/// identification; random identification beside it.
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r4_study_pulse_id_level() {
    let mut benches: Vec<Bench> = engine_plants().iter().map(jointed_bench).collect();
    if std::env::var("R4_PLANT").is_err() {
        benches.push(research_bench());
        benches.push(spring_bench(0.16));
    }
    println!("\n| plant | random id: first at -12 | id at | codes | shots worst abs | first at -12 | first at id + 6 dB | mismatch at -12 |");
    println!("|---|---|---|---|---|---|---|---|");
    for b in &benches {
        let h = b.ms.plant.frf.clone();
        let (o_rand, _) = b.judge(&b.play(&b.seed(&h, -12.0), -12.0), -12.0);
        for (&lid, cname) in [-30.0, -24.0, -18.0, -15.0, -24.0, -18.0].iter().zip([
            "coded",
            "coded",
            "coded",
            "coded",
            "one drive at a time",
            "one drive at a time",
        ]) {
            let codes = if cname == "coded" {
                hadamard4()
            } else {
                one_at_a_time()
            };
            let (m, abs) = pulse_id_with(b, lid, &codes);
            let d12 = b.seed(&m, -12.0);
            let y12 = b.play(&d12, -12.0);
            let (o12, _) = b.judge(&y12, -12.0);
            let up = lid + 6.0;
            let (o6, _) = b.judge(&b.play(&b.seed(&m, up), up), up);
            println!(
                "| {} | {o_rand:+.2} | {lid:+.0} | {cname} | {abs:+.1} | {o12:+.2} | {o6:+.2} | {:.3} |",
                b.name,
                rel(&y12, &predict(&m, &d12))
            );
        }
    }
}

/// MS-R4-07: the stall. The family at 12 % on the linear and moderate
/// plants, 20 pulses at -12 dB (direct harness): `shape` as built, with the
/// level weight at 1 and the step limit at 12 dB (the allowance and rate),
/// and with the drop narrowed by the evidence (the provider started at 12 %,
/// narrowing 0.7 a pulse); with the family's bias on the identification and
/// its coupling at the worst point.
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r4_study_stall() {
    let plants = match std::env::var("R4_PLANT").as_deref() {
        Ok(_) => engine_plants(),
        Err(_) => vec![
            shock(ShockPlant::Linear4x4),
            shock(ShockPlant::Jointed4x4Moderate),
        ],
    };
    for p in &plants {
        let b = jointed_bench(p);
        let h = b.ms.plant.frf.clone();
        let t = b.ms.srs.as_ref().expect("an SRS target").clone();
        // The bias: the family 12 % drive through the identification itself.
        let fam = family_of(&b.ms, &h, 0.12);
        let d = b.seed(&fam, -12.0);
        let yb = predict(&h, &d);
        let mb = srs_rows(&yb, b.ms.fs_hz, &t.natural_hz, t.q, level_scale(-12.0));
        let mut worst = (0.0f64, 0usize, 0usize);
        for (c, (mr, tr)) in mb.iter().zip(&t.points).enumerate() {
            for (k, e) in error_db(mr, tr).into_iter().enumerate() {
                if e.abs() > worst.0.abs() {
                    worst = (e, c, k);
                }
            }
        }
        // The coupling of H G at the worst point's frequency: the off-diagonal
        // share of |H G|^2 on the lines within 5 % of it.
        let g = family_inverse(
            &stretched_family(&warp_on(&b.ms, &h), &[0.0, 0.06, 0.12]),
            &[1.0; 3],
            &b.mask(),
            1e-3,
        )
        .expect("a family inverse");
        let fw = t.natural_hz[worst.2];
        let df = b.ms.fs_hz / b.ms.record_len as f64;
        let (mut diag, mut off, mut dmin) = (0.0, 0.0, f64::INFINITY);
        for i in 0..=b.ms.record_len / 2 {
            let f = i as f64 * df;
            if (f - fw).abs() > 0.05 * fw {
                continue;
            }
            for r in 0..4 {
                for c in 0..4 {
                    let mut s = C64::new(0.0, 0.0);
                    for k in 0..4 {
                        s += h.get(i, r, k) * g.get(i, k, c);
                    }
                    if r == c {
                        diag += s.norm_sqr();
                        dmin = dmin.min(s.norm());
                    } else {
                        off += s.norm_sqr();
                    }
                }
            }
        }
        println!(
            "\n{}: the 12 % family's first drive through the identification: worst SRS error {:+.2} dB at control {} {:.0} Hz; H G there: off-diagonal share {:.3}, smallest diagonal {:.3}\n",
            b.name,
            worst.0,
            worst.1 + 1,
            fw,
            off / (diag + off),
            dmin
        );
        let fast = ShapeParams {
            weights: vec![(-12.0, 1.0), (0.0, 1.0)],
            max_step_db: 12.0,
            ..ShapeParams::default()
        };
        let variants: Vec<(String, Method)> = vec![
            ("frozen".into(), Method::frozen()),
            ("family 12 %".into(), Method::family(0.12)),
            (
                "family 12 %, weight 1, step 12 dB".into(),
                Method {
                    params: fast.clone(),
                    ..Method::family(0.12)
                },
            ),
            ("family 6 %".into(), Method::family(0.06)),
            (
                "family 12 % narrowed by the evidence (0.7 a pulse)".into(),
                Method {
                    softening: Some((plan(SLOW), Some(0.12))),
                    ..Method::frozen()
                },
            ),
        ];
        println!("| variant | worst error by pulse, dB (sign: over +) | at Hz |");
        println!("|---|---|---|");
        for (name, m) in variants {
            let mut law = m.law(&b.ms).expect("a law");
            let mut out = law.seed(-12.0).expect("seeded");
            let mut cells = Vec::new();
            let mut at = 0.0;
            for k in 0..20 {
                let y = b.play(&out.drives, -12.0);
                let srs = srs_rows(&y, b.ms.fs_hz, &t.natural_hz, t.q, level_scale(-12.0));
                let mut w = (0.0f64, 0.0);
                for (mr, tr) in srs.iter().zip(&t.points) {
                    for (j, e) in error_db(mr, tr).into_iter().enumerate() {
                        if e.abs() > w.0.abs() {
                            w = (e, t.natural_hz[j]);
                        }
                    }
                }
                if k % 2 == 0 || k == 19 {
                    cells.push(format!("{:+.2}", w.0));
                }
                at = w.1;
                out = law
                    .iterate(&math::PulseInputs {
                        pulse_index: k,
                        level_db: -12.0,
                        next_level_db: -12.0,
                        drives: out.drives.clone(),
                        responses: y,
                        srs: None,
                    })
                    .expect("iterated");
            }
            println!("| {name} | {} | {at:.0} |", cells.join(" "));
            flush(&m);
        }
    }
}

/// MS-R4-08: the cost of being wrong. The family at a fixed drop `ratio x`
/// the plant's true peak drop at -12 dB (absolute drops on the linear
/// plant), default ladder, six a level.
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r4_study_cost_of_wrong() {
    let ratios = [0.0, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0];
    let def = LadderPlan::default().levels_db;
    for p in engine_plants() {
        let b = jointed_bench(&p);
        let truth = first_mode_drop(
            &p.jp,
            &b.seed(&family_of(&b.ms, &b.ms.plant.frf, 0.08), -12.0),
        );
        let drops: Vec<(String, f64)> = if truth > 1e-4 {
            ratios
                .iter()
                .map(|r| {
                    (
                        format!("ratio {r:.2} ({:.1} %)", 100.0 * r * truth),
                        r * truth,
                    )
                })
                .collect()
        } else {
            [0.0, 0.02, 0.04, 0.06, 0.08, 0.12, 0.16, 0.24]
                .iter()
                .map(|d| (format!("{:.0} %", 100.0 * d), *d))
                .collect()
        };
        header(&format!(
            "{}: true peak drop at -12 dB {:.2} %",
            p.name,
            100.0 * truth
        ));
        for (name, d) in drops {
            let r = engine_run(&p, Arc::new(Method::family(d)), def.clone(), 6, -12.0);
            print_run(&name, 0, &r);
        }
    }
    use math::reference_plant::LADDER_DB;
    let spring_ladder: Vec<f64> = def.iter().copied().chain([0.0; 7]).collect();
    for (b, lad, truth) in [
        (
            research_bench(),
            LADDER_DB.to_vec(),
            0.08 * level_scale(-12.0),
        ),
        (spring_bench(0.16), spring_ladder, 0.08),
    ] {
        header(&format!(
            "{}: true drop at -12 dB about {:.2} %",
            b.name,
            100.0 * truth
        ));
        for r in ratios {
            let run = runner_run(&b, &Method::family(r * truth), &lad, -12.0);
            print_run(&format!("ratio {r:.2}"), 0, &run);
        }
    }
}

/// The inverse law with every answer's growth over the drive just played
/// recorded (the engine's own measure).
struct GrowthLog {
    inner: InverseFactory,
    log: Arc<Mutex<Vec<String>>>,
}

struct GrowthLaw {
    inner: Box<dyn hertzforge_mimo_shock::MimoShockLaw + Send>,
    len: usize,
    bands: Vec<(usize, usize)>,
    log: Arc<Mutex<Vec<String>>>,
}

impl hertzforge_mimo_shock::MimoShockLaw for GrowthLaw {
    fn name(&self) -> &str {
        "inverse-logged"
    }
    fn seed(&mut self, level_db: f64) -> Result<hertzforge_mimo_shock::PulseOutputs, Fault> {
        self.inner.seed(level_db)
    }
    fn iterate(
        &mut self,
        i: &hertzforge_mimo_shock::PulseInputs,
    ) -> Result<hertzforge_mimo_shock::PulseOutputs, Fault> {
        let o = self.inner.iterate(i)?;
        let step = level_scale(i.next_level_db) / level_scale(i.level_db);
        if let Ok(g) = hertzforge_mimo_shock::guards::growth(
            &o.drives,
            &i.drives,
            self.len,
            step,
            &self.bands,
            hertzforge_mimo_shock::engine::GROWTH_BAND_FLOOR,
        ) {
            let w = g
                .iter()
                .max_by(|a, b| a.band_db.total_cmp(&b.band_db))
                .expect("a drive");
            self.log.lock().expect("log").push(format!(
                "pulse {} at {:+.1}: band growth {:+.1} dB at drive {} near {:.0} Hz, peak growth {:+.1} dB",
                i.pulse_index,
                i.level_db,
                w.band_db,
                w.drive + 1,
                w.band_line as f64 * FS / self.len as f64,
                g.iter().fold(f64::NEG_INFINITY, |a, x| a.max(x.peak_db))
            ));
        }
        Ok(o)
    }
}

impl MimoShockLawFactory for GrowthLog {
    fn build(
        &self,
        setup: &hertzforge_mimo_shock::Setup,
    ) -> Result<Box<dyn hertzforge_mimo_shock::MimoShockLaw + Send>, Fault> {
        Ok(Box::new(GrowthLaw {
            inner: self.inner.build(setup)?,
            len: setup.record_len,
            bands: hertzforge_mimo_shock::guards::bands(FS, setup.record_len, 5.0, 2500.0),
            log: self.log.clone(),
        }))
    }
    fn describe(&self) -> hertzforge_shock_common::law_guard::LawDescriptor {
        self.inner.describe()
    }
    fn manifest(&self) -> hertzforge_mimo_shock::LawManifest {
        self.inner.manifest()
    }
}

/// MS-R4-09: the classical twist target on the severe and linear plants,
/// `inverse` at its defaults, with the band growth cap at 10 dB (the
/// default), 16, 24 dB and off: what growth the law asks for, where, and how
/// the run ends.
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r4_study_classical_twist() {
    let plants = match std::env::var("R4_PLANT").as_deref() {
        Ok(_) => engine_plants(),
        Err(_) => vec![
            shock(ShockPlant::Jointed4x4Severe),
            shock(ShockPlant::Jointed4x4Moderate),
            shock(ShockPlant::Linear4x4),
        ],
    };
    for p in &plants {
        println!("\n{}, twist classical, inverse at its defaults\n", p.name);
        for cap in [10.0, 16.0, 24.0, 1000.0] {
            let log: Arc<Mutex<Vec<String>>> = Arc::default();
            let mut cfg = config(4, 0, 4, LEN_CLASSICAL);
            cfg.guards.max_band_growth_db = cap;
            let mut e = hertzforge_mimo_shock::MimoShockEngine::new(
                cfg,
                Arc::new(GrowthLog {
                    inner: InverseFactory::default(),
                    log: log.clone(),
                }),
            )
            .expect("a configuration")
            .with_target_spec(classical_twist())
            .with_clock(Arc::new(ManualClock::default()))
            .with_pacing(false);
            e.arm(
                station_of(p, seed_env()),
                table(4, 0, 4),
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
            let pulses = e.pulses();
            let overs: Vec<String> = pulses
                .iter()
                .map(|q| format!("{:+.0}:{:+.2}", q.level_db, q.over_test_db))
                .collect();
            println!(
                "cap {cap:.0} dB: {} pulses, end {}; level:over {}",
                pulses.len(),
                e.end()
                    .map(|x| format!("{:?} {} ({})", x.outcome, x.reason, x.detail))
                    .unwrap_or_else(|| "none".into()),
                overs.join(" ")
            );
            for s in log.lock().expect("log").iter() {
                println!("    {s}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The operator-free part (MS-R4-10 onward).
// ---------------------------------------------------------------------------

/// The operator-free setting (written in the experiment log before the
/// held-out plant ran): the first mapping pulse on the identification, every
/// later pulse on the family; the margin measured; the power measured with no
/// bound; no ceiling; widened on steps, narrowed to the level's own fit inside
/// a level with the law's gains reset when it narrows.
fn operator_free() -> Method {
    Method {
        carry: CarryGain::ResetOnOwn,
        ..Method::soft(SofteningPlan {
            probe_levels: 1,
            follow: Follow::Narrowing { narrow: 0.0 },
            data_margin: true,
            p_max: f64::INFINITY,
            // Not an operator number: the largest drop the fit can measure.
            ceiling: FitOptions::default().coarse.1,
            ladder_start_db: Some(-12.0),
            ..SofteningPlan::default()
        })
    }
}

/// The operator-free setting without the narrowing inside a level: only
/// ever widened (round 4's following) with the measured margin and power and
/// no ceiling.
fn operator_free_widen() -> Method {
    let mut m = operator_free();
    m.carry = CarryGain::Keep;
    if let Some((plan, _)) = m.softening.as_mut() {
        plan.follow = WIDEN;
    }
    m
}

/// Mapping sets of R4-I.
fn mapping_sets() -> Vec<(&'static str, Vec<f64>)> {
    let all = vec![
        ("-40 -34 -28 -22", vec![-40.0, -34.0, -28.0, -22.0]),
        ("-40 -30 -20", vec![-40.0, -30.0, -20.0]),
        ("-34 -28 -22 -18", vec![-34.0, -28.0, -22.0, -18.0]),
        ("-40 -34", vec![-40.0, -34.0]),
    ];
    match std::env::var("R4_MAP")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        Some(k) => vec![all[k].clone()],
        None => all,
    }
}

fn mapped(map: &[f64], l: &[f64]) -> Vec<f64> {
    map.iter().copied().chain(l.iter().copied()).collect()
}

/// MS-R4-10 to -12: the operator-free scheme beside round 4's ceiling-based
/// "followed" and the baselines, on the engine's plants (`R4_PLANT`, `late`
/// for the held-out plant) and the runner plants (`R4_RUNNER`: dev, heldout,
/// none), with `R4_NOISE` relative noise and `R4_MAP` one mapping set.
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_r4b_study_operator_free() {
    let def = LadderPlan::default().levels_db;
    let base_rows = std::env::var("R4_BASELINES")
        .map(|v| v != "0")
        .unwrap_or(true);
    println!("relative noise {:e}", noise_rel());
    let run_engine = std::env::var("R4_RUNNER")
        .map(|v| v == "none")
        .unwrap_or(true)
        || std::env::var("R4_PLANT").is_ok();
    if run_engine {
        for p in engine_plants() {
            header(&format!(
                "{}, twist SRS, station seed {}",
                p.name,
                seed_env()
            ));
            let mut rows: Vec<(String, Method, Vec<f64>)> = Vec::new();
            if base_rows {
                rows.push((
                    "frozen, default ladder".into(),
                    Method::frozen(),
                    def.clone(),
                ));
                rows.push((
                    "frozen, -40 by 3".into(),
                    Method::frozen(),
                    ladder(-40.0, 3.0),
                ));
                rows.push((
                    "family 8 % (hand-set)".into(),
                    Method::family(0.08),
                    def.clone(),
                ));
                rows.push((
                    "round 4 followed, ceiling 12 %".into(),
                    Method::soft(plan(WIDEN)),
                    with_probes(&def),
                ));
            }
            for (name, map) in mapping_sets() {
                rows.push((
                    format!("operator-free, mapping {name}"),
                    operator_free(),
                    mapped(&map, &def),
                ));
                rows.push((
                    format!("operator-free widen only, mapping {name}"),
                    operator_free_widen(),
                    mapped(&map, &def),
                ));
            }
            for (name, m, lad) in rows {
                let r = engine_run(&p, Arc::new(m.clone()), lad, 6, -12.0);
                print_run(&name, 0, &r);
                flush(&m);
            }
        }
    }
    use math::reference_plant::LADDER_DB;
    let which = std::env::var("R4_RUNNER").unwrap_or_else(|_| "none".into());
    if which == "none" {
        return;
    }
    let spring_ladder: Vec<f64> = def.iter().copied().chain([0.0; 7]).collect();
    let mut benches: Vec<(Bench, Vec<f64>)> = Vec::new();
    if which == "dev" || which == "all" {
        benches.push((research_bench(), LADDER_DB.to_vec()));
        benches.push((spring_bench(0.16), spring_ladder.clone()));
        benches.push((spring_bench(0.32), spring_ladder.clone()));
    }
    for (b, lad) in &benches {
        header(&format!("{} (research ladder runner)", b.name));
        let mut rows: Vec<(String, Method, Vec<f64>)> = Vec::new();
        if base_rows {
            rows.push(("frozen, from -12".into(), Method::frozen(), lad.clone()));
            rows.push((
                "frozen, -40 by 3".into(),
                Method::frozen(),
                ladder(-40.0, 3.0).into_iter().chain([0.0; 7]).collect(),
            ));
            rows.push((
                "family 8 % (hand-set)".into(),
                Method::family(0.08),
                lad.clone(),
            ));
            rows.push((
                "round 4 followed, ceiling 12 %".into(),
                Method::soft(plan(WIDEN)),
                with_probes(lad),
            ));
        }
        for (name, map) in mapping_sets() {
            rows.push((
                format!("operator-free, mapping {name}"),
                operator_free(),
                mapped(&map, lad),
            ));
            rows.push((
                format!("operator-free widen only, mapping {name}"),
                operator_free_widen(),
                mapped(&map, lad),
            ));
        }
        for (name, m, l) in rows {
            let r = runner_run(b, &m, &l, -12.0);
            print_run(&name, 0, &r);
            flush(&m);
        }
    }
}
