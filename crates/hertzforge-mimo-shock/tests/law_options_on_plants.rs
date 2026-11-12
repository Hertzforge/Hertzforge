//! The built-in laws' model options run through the engine on the
//! simulator's standard shock plants: the study the defaults rest on.
//!
//! Each row is one whole run, identification to the end, through the host
//! firer, at the engine's defaults except where the row says (the law's
//! `model` and options, the ladder, the level-scaled aborts). The rows are
//! printed; the requirements below are asserted by the tests that name them,
//! and the study tests only print.
//!
//! Requirements, fixed before the first run (rate 12 800 Hz, SRS records
//! 16384 samples and classical 8192, targets as the engine's run tests:
//! the twist targets drive the plate's first, jointed mode):
//!
//! - X1 (severe 4 x 4, twist SRS target): with the improved `shape`
//!   (`model = track`, gain clamp 12 dB) the first full-level pulse's signed
//!   over-test is at most +1 dB on every control (the research round's
//!   number on its own plants), and the run never ends on an abort line.
//! - X2 (the same): the third full-level pulse's worst SRS error is inside the
//!   target's tolerance (3 dB).
//! - X3 (severe 4 x 4, twist classical target): with the improved `inverse`
//!   (`model = track_secant`, predicted level step) the first full-level
//!   pulse's peak is at most +1 dB over its target's, and the run never ends
//!   on an abort line.
//!
//! The columns: the first pulse's signed over-test (`p1`), the first
//! full-level pulse's (`f1`), the worst error of the third full-level pulse
//! (`f3 err`, dB: the SRS deviation for an SRS target, the waveform error for
//! a classical one), the pulses fired, and the end.

mod support;

use hertzforge_hal_sim::plants::ShockPlant;
use hertzforge_law_sdk::shock_mimo::builtin::ModelChoice;
use hertzforge_mimo_shock::builtin::{InverseFactory, ShapeFactory};
use hertzforge_mimo_shock::{LadderPlan, MimoShockLawFactory, TargetSpec};
use std::sync::Arc;
use support::*;

/// One run's numbers.
#[derive(Debug, Clone)]
pub struct Row {
    pub p1_db: f64,
    pub f1_db: Option<f64>,
    pub f3_err_db: Option<f64>,
    pub pulses: usize,
    pub end: String,
    pub aborted: bool,
    pub id_db: f64,
}

fn starter_ladder() -> Vec<f64> {
    vec![-18.0, -12.0, -9.0, -6.0, -3.0, -1.5, 0.0]
}

fn plain_ladder() -> Vec<f64> {
    vec![-12.0, -9.0, -6.0, -3.0, -1.5, 0.0]
}

fn run(
    plant: ShockPlant,
    spec: TargetSpec,
    len: usize,
    law: Arc<dyn MimoShockLawFactory>,
    ladder: Vec<f64>,
    scaled: bool,
) -> Row {
    let (nc, nd) = plant.dims();
    let mut cfg = config(nc, 0, nd, len);
    cfg.ladder = LadderPlan {
        levels_db: ladder,
        ..LadderPlan::default()
    };
    cfg.guards.scaled_aborts = scaled;
    let clock = Arc::new(ManualClock::default());
    let mut e = hertzforge_mimo_shock::MimoShockEngine::new(cfg, law)
        .expect("a good configuration")
        .with_target_spec(spec)
        .with_clock(clock)
        .with_pacing(false);
    e.arm(
        station(plant, FS, 11),
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
    if std::env::var("MSX_TRACE").is_ok() {
        println!("{}", trace(&e));
        for q in e.pulses() {
            let v: Vec<String> = q
                .law_values
                .iter()
                .map(|x| format!("{}={:.3}", x.key, x.value))
                .collect();
            println!("  values after {}: {}", q.index, v.join(" "));
        }
    }
    let p = e.pulses();
    let full: Vec<_> = p.iter().filter(|q| q.level_db == 0.0).collect();
    let worst = |q: &hertzforge_mimo_shock::PulseReport| {
        q.controls
            .iter()
            .fold(0.0f64, |a, c| a.max(c.verdict.error_db.abs()))
    };
    let end = e.end().cloned();
    Row {
        p1_db: p.first().map(|q| q.over_test_db).unwrap_or(f64::NAN),
        f1_db: full.first().map(|q| q.over_test_db),
        f3_err_db: full.get(2).map(|q| worst(q)),
        pulses: p.len(),
        aborted: end
            .as_ref()
            .map(|x| x.reason.contains("abort"))
            .unwrap_or(false),
        id_db: e
            .setup()
            .and_then(|x| x.identification_level_db)
            .unwrap_or(f64::NAN),
        end: end
            .map(|x| format!("{:?} {}", x.outcome, x.reason))
            .unwrap_or_else(|| "none".into()),
    }
}

fn fmt(o: Option<f64>) -> String {
    o.map(|v| format!("{v:+6.2}"))
        .unwrap_or_else(|| "    - ".into())
}

fn print(label: &str, r: &Row) {
    println!(
        "| {label:<34} | {:+6.1} | {:+6.2} | {} | {} | {:>3} | {} |",
        r.id_db,
        r.p1_db,
        fmt(r.f1_db),
        fmt(r.f3_err_db),
        r.pulses,
        r.end
    );
}

/// `shape` with `model` and the 12 dB clamp (or the frozen default).
pub fn shape(model: ModelChoice, clamp_db: f64) -> ShapeFactory {
    let mut f = ShapeFactory::default();
    f.model.model = model;
    f.gain_clamp_db = clamp_db;
    f
}

/// `inverse` with `model`, the predicted step on when asked.
pub fn inverse(model: ModelChoice, step: bool) -> InverseFactory {
    let mut f = InverseFactory::default();
    f.model.model = model;
    f.predicted_step = step;
    f
}

type Rows = Vec<(String, Arc<dyn MimoShockLawFactory>)>;

fn shape_rows() -> Rows {
    let mut id_point = shape(ModelChoice::Track, 12.0);
    id_point.model.from_identification = true;
    vec![
        (
            "shape frozen".into(),
            Arc::new(shape(ModelChoice::Frozen, 0.0)),
        ),
        (
            "shape frozen clamp".into(),
            Arc::new(shape(ModelChoice::Frozen, 12.0)),
        ),
        (
            "shape secant clamp".into(),
            Arc::new(shape(ModelChoice::Secant, 12.0)),
        ),
        (
            "shape track clamp".into(),
            Arc::new(shape(ModelChoice::Track, 12.0)),
        ),
        (
            "shape track_secant clamp".into(),
            Arc::new(shape(ModelChoice::TrackSecant, 12.0)),
        ),
        ("shape track clamp, id point".into(), Arc::new(id_point)),
    ]
}

fn inverse_rows() -> Rows {
    vec![
        (
            "inverse frozen".into(),
            Arc::new(inverse(ModelChoice::Frozen, false)),
        ),
        (
            "inverse secant".into(),
            Arc::new(inverse(ModelChoice::Secant, false)),
        ),
        (
            "inverse track".into(),
            Arc::new(inverse(ModelChoice::Track, false)),
        ),
        (
            "inverse track step".into(),
            Arc::new(inverse(ModelChoice::Track, true)),
        ),
        (
            "inverse track_secant step".into(),
            Arc::new(inverse(ModelChoice::TrackSecant, true)),
        ),
    ]
}

fn header(title: &str) {
    println!("\n{title}\n");
    println!("| law and options | id | p1 | f1 | f3 err | n | end |");
    println!("|---|---|---|---|---|---|---|");
}

fn study(plant: ShockPlant, target: &str, spec: impl Fn() -> TargetSpec, len: usize, rows: Rows) {
    let only = std::env::var("MSX_ONLY").ok();
    let mut ladders = vec![("starter", starter_ladder()), ("plain", plain_ladder())];
    if let Ok(l) = std::env::var("MSX_LADDER") {
        ladders = vec![(
            "given",
            l.split(',')
                .map(|x| x.trim().parse().expect("a level"))
                .collect(),
        )];
    }
    let scaled_set: Vec<bool> = match std::env::var("MSX_SCALED").as_deref() {
        Ok("on") => vec![true],
        Ok("off") => vec![false],
        _ => vec![true, false],
    };
    for (ladder_name, ladder) in ladders {
        for &scaled in &scaled_set {
            header(&format!(
                "{} {target}, ladder {ladder_name}, scaled aborts {}",
                plant.name(),
                if scaled { "on" } else { "off" }
            ));
            for (name, law) in &rows {
                if let Some(o) = &only {
                    // Names separated by `;`, each matched exactly.
                    if !o.split(';').any(|x| x == name) {
                        continue;
                    }
                }
                let r = run(plant, spec(), len, law.clone(), ladder.clone(), scaled);
                print(name, &r);
            }
        }
    }
}

#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_x_study_severe_twist_srs() {
    let mut rows = shape_rows();
    rows.extend(inverse_rows());
    study(
        ShockPlant::Jointed4x4Severe,
        "twist SRS",
        srs_twist,
        LEN_SRS,
        rows,
    );
}

#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_x_study_severe_twist_classical() {
    study(
        ShockPlant::Jointed4x4Severe,
        "twist classical",
        classical_twist,
        LEN_CLASSICAL,
        inverse_rows(),
    );
}

#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_x_study_other_plants() {
    for plant in [
        ShockPlant::Linear4x4,
        ShockPlant::Jointed4x4Moderate,
        ShockPlant::Jointed6x3,
    ] {
        let (nc, _) = plant.dims();
        let flat: Vec<f64> = (0..nc)
            .map(|c| if c % 2 == 0 { 20.0 } else { 16.0 })
            .collect();
        let mut rows = shape_rows();
        rows.extend(inverse_rows());
        if nc == 4 {
            study(plant, "twist SRS", srs_twist, LEN_SRS, rows.clone());
            study(
                plant,
                "twist classical",
                classical_twist,
                LEN_CLASSICAL,
                inverse_rows(),
            );
        }
        study(plant, "in-phase SRS", || srs(&flat), LEN_SRS, rows);
    }
}

/// The modal fit on each plant's identification: the modes counted, and the
/// fit's residual for a range of mode counts.
#[test]
#[ignore = "a study: run on demand, prints its table"]
fn mimo_shock_x_study_modal_fit_on_the_identifications() {
    use hertzforge_law_sdk::shock_mimo::builtin::{count_modes, ShapeConfig};
    for plant in ShockPlant::ALL {
        let (nc, nd) = plant.dims();
        let cfg = config(nc, 0, nd, LEN_SRS);
        let flat: Vec<f64> = (0..nc).map(|_| 20.0).collect();
        let mut e = hertzforge_mimo_shock::MimoShockEngine::new(
            cfg,
            Arc::new(shape(ModelChoice::Frozen, 0.0)),
        )
        .unwrap()
        .with_target_spec(srs(&flat))
        .with_clock(Arc::new(ManualClock::default()))
        .with_pacing(false);
        e.arm(
            station(plant, FS, 11),
            table(nc, 0, nd),
            hertzforge_module_abi::StationClass::HertzforgeSim,
        )
        .unwrap();
        e.identify().unwrap();
        let setup = e.setup().unwrap().clone();
        let ms = hertzforge_mimo_shock::builtin::math_setup(&setup);
        let band = hertzforge_mimo_shock_law_math::drive_mask(&ms);
        println!(
            "{}: {} trusted lines, {} modes counted",
            plant.name(),
            band.iter().filter(|b| **b).count(),
            count_modes(&ms.plant.frf, &band)
        );
        for modes in [0usize, 4, 6, 8, 10, 12] {
            let mut c = ShapeConfig::default();
            c.model.model = ModelChoice::Track;
            c.model.modes = modes;
            let t0 = std::time::Instant::now();
            match c.build(ms.clone(), f64::NAN) {
                Ok(l) => println!(
                    "  modes {modes:>2}: {:?} in {:.2} s",
                    l.model_build(),
                    t0.elapsed().as_secs_f64()
                ),
                Err(err) => println!("  modes {modes:>2}: {err}"),
            }
        }
    }
}

/// The math law with the model diagnostics the built-in's eight values leave
/// out: how far the identification (frozen) and the same-level secant would
/// have missed each pulse, beside the model that played it.
struct Probe {
    model: ModelChoice,
}

struct ProbeLaw {
    law: Box<dyn hertzforge_mimo_shock_law_math::LawSteps + Send>,
    len: usize,
    nf: usize,
}

const PROBE_KEYS: [&str; 6] = [
    "worst_srs_error_db",
    "model_prediction_residual",
    "model_fallback_residual",
    "model_fit_residual",
    "model_source",
    "weight",
];

fn probe_out(
    o: hertzforge_mimo_shock_law_math::PulseOutputs,
) -> hertzforge_mimo_shock::PulseOutputs {
    hertzforge_mimo_shock::PulseOutputs {
        drives: o.drives.concat(),
        delays_s: o.delays_s,
        values: o
            .values
            .into_iter()
            .filter(|(k, _)| PROBE_KEYS.contains(k))
            .map(|(k, v)| hertzforge_shock_common::law_guard::LawValue::new(k, v))
            .collect(),
    }
}

impl hertzforge_mimo_shock::MimoShockLaw for ProbeLaw {
    fn name(&self) -> &str {
        "probe"
    }
    fn seed(
        &mut self,
        level_db: f64,
    ) -> Result<hertzforge_mimo_shock::PulseOutputs, hertzforge_shock_common::law_guard::LawFault>
    {
        self.law
            .seed(level_db)
            .map(probe_out)
            .map_err(hertzforge_shock_common::law_guard::LawFault::Failed)
    }
    fn iterate(
        &mut self,
        i: &hertzforge_mimo_shock::PulseInputs,
    ) -> Result<hertzforge_mimo_shock::PulseOutputs, hertzforge_shock_common::law_guard::LawFault>
    {
        let rows = |f: &[f64], n: usize| f.chunks(n.max(1)).map(|c| c.to_vec()).collect();
        let m = hertzforge_mimo_shock_law_math::PulseInputs {
            pulse_index: i.pulse_index as usize - 1,
            level_db: i.level_db,
            next_level_db: i.next_level_db,
            drives: rows(&i.drives, self.len),
            responses: rows(&i.responses, self.len),
            srs: i.srs.as_ref().map(|s| rows(s, self.nf)),
        };
        self.law
            .iterate(&m)
            .map(probe_out)
            .map_err(hertzforge_shock_common::law_guard::LawFault::Failed)
    }
}

impl MimoShockLawFactory for Probe {
    fn build(
        &self,
        setup: &hertzforge_mimo_shock::Setup,
    ) -> Result<
        Box<dyn hertzforge_mimo_shock::MimoShockLaw + Send>,
        hertzforge_shock_common::law_guard::LawFault,
    > {
        use hertzforge_law_sdk::shock_mimo::builtin::ModelOptions;
        use hertzforge_mimo_shock_law_math::shape::{ShapeExtras, ShapeLaw, ShapeParams};
        let ms = hertzforge_mimo_shock::builtin::math_setup(setup);
        let opts = ModelOptions {
            model: self.model,
            ..ModelOptions::default()
        };
        let (provider, _) = opts
            .provider(&ms, setup.identification_level_db.unwrap_or(f64::NAN))
            .map_err(hertzforge_shock_common::law_guard::LawFault::Failed)?;
        let provider = provider.unwrap_or_else(|| {
            Box::new(hertzforge_mimo_shock_law_math::provider::FrozenModel::new(
                ms.plant.frf.clone(),
                hertzforge_mimo_shock_law_math::drive_mask(&ms),
            ))
        });
        let nf = match &setup.target {
            hertzforge_mimo_shock::Target::Srs(t) => t.natural_hz.len(),
            _ => 0,
        };
        let law = ShapeLaw::with_options(
            ms,
            ShapeParams::default(),
            ShapeExtras {
                provider: Some(provider),
                ..Default::default()
            },
        )
        .map_err(hertzforge_shock_common::law_guard::LawFault::Failed)?;
        Ok(Box::new(ProbeLaw {
            law: Box::new(law),
            len: setup.record_len,
            nf,
        }))
    }
    fn describe(&self) -> hertzforge_shock_common::law_guard::LawDescriptor {
        ShapeFactory::default().describe()
    }
    fn manifest(&self) -> hertzforge_mimo_shock::LawManifest {
        hertzforge_mimo_shock::LawManifest {
            targets: vec![hertzforge_mimo_shock::TargetKind::Srs],
            values: PROBE_KEYS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// How linear the plant is from pulse to pulse: per pulse, the relative miss
/// of the model that played it, of the identification, and of the secant.
#[test]
#[ignore = "a study: run on demand, prints its trace"]
fn mimo_shock_x_study_model_misses_on_the_severe_plant() {
    let ladder: Vec<f64> = std::env::var("MSX_LADDER")
        .unwrap_or_else(|_| "-40,-30,-24,-18,-12,-6,-3,0".into())
        .split(',')
        .map(|x| x.trim().parse().unwrap())
        .collect();
    for model in [ModelChoice::Frozen, ModelChoice::Track] {
        let r = {
            std::env::set_var("MSX_TRACE", "1");
            run(
                ShockPlant::Jointed4x4Severe,
                srs_twist(),
                LEN_SRS,
                Arc::new(Probe { model }),
                ladder.clone(),
                false,
            )
        };
        print(model.name(), &r);
    }
}
