//! Evidence for the structure delay `matrix`'s loop-speed rule reads, through
//! the engine on the simulator, ignored by default (minutes): `largest` and
//! `principal` on the same noise seeds, at the closed-loop tests' noise.
//! Dwells started 6 dB low (time to within 1 dB), a stepped sine and sweeps at
//! one and four octaves a minute: outcome, worst and median error, and the
//! largest drive against its 2 V ceiling. `MSINE_N_STRUCTURES` (comma list of
//! `linear`, `mild`, `moderate`) and `MSINE_N_SEEDS` (count) narrow the run.

mod common;

use common::*;
use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_law_sdk::sine_mimo::builtin as decl;
use hertzforge_law_sdk::ParamValue;
use hertzforge_mimo_sine::law::{LawDescriptor, LawManifest, LawSetup};
use hertzforge_mimo_sine::laws::BuiltinFactory;
use hertzforge_mimo_sine::service::{EngineOptions, MimoSineService};
use hertzforge_mimo_sine::*;
use hertzforge_module_abi::StationClass;
use hertzforge_sine::scheduler::{
    BidirSweepConfig, DwellFor, StepSineConfig, StepSpacing, SweepDirection, SweepTarget,
};
use hertzforge_sine::SweepRate;
use std::sync::Arc;

/// `matrix` with `delay_estimate` set.
fn matrix(how: &str) -> Arc<dyn MimoSineLawFactory> {
    let mut p = decl::defaults(&decl::MATRIX);
    for x in p.0.iter_mut() {
        if x.key == "delay_estimate" {
            x.value = ParamValue::Choice(how.into());
        }
    }
    Arc::new(BuiltinFactory::new(&decl::MATRIX, p).unwrap())
}

/// A law whose first drive is half of what it asks: 6 dB low.
struct Low(Arc<dyn MimoSineLawFactory>);
struct LowLaw(Box<dyn MimoSineLaw + Send>);

impl MimoSineLaw for LowLaw {
    fn name(&self) -> &str {
        "low"
    }
    fn engage(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut law::LawValues,
    ) -> Result<(), LawFault> {
        self.0.engage(x, o, v)?;
        o.drive.iter_mut().for_each(|d| *d *= 0.5);
        Ok(())
    }
    fn tick(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut law::LawValues,
    ) -> Result<(), LawFault> {
        self.0.tick(x, o, v)
    }
}

impl MimoSineLawFactory for Low {
    fn build(&self, s: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault> {
        Ok(Box::new(LowLaw(self.0.build(s)?)))
    }
    fn describe(&self) -> LawDescriptor {
        self.0.describe()
    }
    fn manifest(&self) -> LawManifest {
        self.0.manifest()
    }
}

fn stepped() -> TestPlan {
    TestPlan::Stepped(StepPlan::Grid(StepSineConfig {
        band: (30.0, 120.0),
        steps_per_octave: 12.0,
        spacing: StepSpacing::Log,
        dwell: Some(DwellFor::Seconds(1.5)),
    }))
}

fn sweep(rate: f64) -> TestPlan {
    TestPlan::Sweep(BidirSweepConfig {
        band: (30.0, 300.0),
        rate: SweepRate::LogOctPerMin(rate),
        start_hz: 30.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(1),
    })
}

fn go(
    s: SineStructure,
    test: TestPlan,
    f: Arc<dyn MimoSineLawFactory>,
    seed: u64,
    max_s: f64,
) -> Run {
    let low = matches!(test, TestPlan::Dwell(_));
    let mut st = setup(s, test, "matrix");
    if low {
        // Started 6 dB low: the abort band must not end the run at engage.
        let g = s.full_level_g();
        for c in &mut st.controls {
            c.profile = flat(g, 1.5, 20.0);
        }
        st.guards.abort_ticks = 20;
    }
    let mut svc = MimoSineService::new(
        st,
        table(s),
        backend(s, NOISE, seed),
        f,
        // On the simulator the law's time budget is relaxed, as the other simulator
        // harnesses relax theirs, so a call a busy machine made late is not a miss here.
        EngineOptions {
            law_budget: Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET),
            ..EngineOptions::new(StationClass::HertzforgeSim)
        },
    )
    .expect("the setup is accepted");
    svc.arm().expect("arms");
    svc.identify().expect("identifies");
    svc.engage().expect("engages");
    run(svc, max_s)
}

/// Outcome, median and worst error over judged updates, largest drive (V).
fn summary(r: &Run) -> String {
    let mut v: Vec<f64> = r
        .ticks
        .iter()
        .filter(|t| !t.settling && t.miss.is_none())
        .flat_map(|t| t.errors_db().iter().map(|e| e.abs()).collect::<Vec<_>>())
        .collect();
    v.sort_by(f64::total_cmp);
    let drive = r
        .ticks
        .iter()
        .flat_map(|t| t.drives().iter().map(|d| d.norm()).collect::<Vec<_>>())
        .fold(0.0, f64::max);
    let (med, worst) = if v.is_empty() {
        (f64::NAN, f64::NAN)
    } else {
        (v[v.len() / 2], v[v.len() - 1])
    };
    let o = match r.outcome {
        Some(records::Outcome::Completed) => "done".to_string(),
        o => format!("{o:?}"),
    };
    format!("{o} {worst:.2}/{med:.2} dB, drive {drive:.2}/2 V")
}

/// Time to within 1 dB on every control (s from engage), the worst error over
/// the last second, and the median gain.
fn dwell_summary(r: &Run) -> String {
    let t0 = r.ticks.first().map_or(0.0, |t| t.time_s);
    let judged: Vec<_> = r
        .ticks
        .iter()
        .filter(|t| t.miss.is_none() && !t.settling)
        .collect();
    let worst = |t: &&records::TickRecord| {
        t.errors_db()
            .iter()
            .map(|e| {
                if e.is_finite() {
                    e.abs()
                } else {
                    f64::INFINITY
                }
            })
            .fold(0.0, f64::max)
    };
    let first = judged
        .iter()
        .find(|t| worst(t) <= 1.0)
        .map(|t| t.time_s - t0);
    let end = judged.last().map_or(0.0, |t| t.time_s);
    let tail = judged
        .iter()
        .filter(|t| t.time_s >= end - 1.0)
        .map(worst)
        .fold(0.0, f64::max);
    let mut g: Vec<f64> = judged
        .iter()
        .map(|t| t.values[0])
        .filter(|g| g.is_finite() && *g > 0.0)
        .collect();
    g.sort_by(f64::total_cmp);
    let gm = g.get(g.len() / 2).copied().unwrap_or(f64::NAN);
    format!(
        "to 1 dB {} s, last 1 s {tail:.2} dB, gain {gm:.4}",
        first.map_or("never".into(), |t| format!("{t:.2}"))
    )
}

#[test]
#[ignore = "evidence: minutes"]
fn mimo_sine_evidence_delay_estimate_through_the_engine() {
    let pick = std::env::var("MSINE_N_STRUCTURES").unwrap_or("linear,mild,moderate".into());
    let seeds: u64 = std::env::var("MSINE_N_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let all = [
        ("linear", SineStructure::Linear4x4),
        ("mild", SineStructure::SofteningMild4x4),
        ("moderate", SineStructure::SofteningModerate4x4),
    ];
    for (name, s) in all.into_iter().filter(|(n, _)| pick.contains(n)) {
        for seed in 7..7 + seeds {
            for how in ["largest", "principal"] {
                let mut line = format!("{name} seed {seed} {how:9}");
                for hz in [100.0, 350.0] {
                    let r = go(s, dwell(hz, 6.0), Arc::new(Low(matrix(how))), seed, 10.0);
                    line += &format!(" | dwell {hz} low: {}, {}", summary(&r), dwell_summary(&r));
                }
                let r = go(s, stepped(), matrix(how), seed, 60.0);
                line += &format!(" | stepped {}", summary(&r));
                for rate in [1.0, 4.0] {
                    let r = go(s, sweep(rate), matrix(how), seed, 250.0);
                    line += &format!(" | sweep {rate} {}", summary(&r));
                }
                println!("{line}");
            }
        }
    }
}
