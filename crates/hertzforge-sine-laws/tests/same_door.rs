//! The same door: the built-in Sine law natively and its law file
//! (`laws/sine-servo`, the same code built for the law host) run the same
//! seeded simulator runs to the same result. Every drive committed and every
//! value the law reported agree within relative 1e-9; the ticks, the notch,
//! the level, the run's end, the trip and the law's counts agree exactly. At
//! the default parameters and at `blend_w` 0 with fast compression (the
//! second set moves the run by more than 1e-3 dB somewhere). The limit
//! channel case is run and reported, not gated: a notch decision on a
//! knife-edge can go either way on a last bit.
//!
//! The runs time the law on a clock that does not move, with the whole tick
//! as the budget: this is about the arithmetic, not the timing (that is
//! `law_call_timing.rs`).

mod common;

use common::{law_file, station_for};
use hertzforge_law_host::{HostConfig, LawHost, Param, ParamValue};
use hertzforge_sine::law::servo_blend::{ServoBlendFactory, ServoBlendParams};
use hertzforge_sine::law::{LawClock, LawGuards, SineLawFactory};
use hertzforge_sine::service::engine::TickObservation;
use hertzforge_sine::{CompressionPreset, RunState, SineService};
use hertzforge_sine_laws::{
    sine_check_config, sine_check_table, SineCheckCase, WasmSineLawFactory, SINE_CHECK_FS_HZ,
    SINE_CHECK_SEED,
};
use std::sync::{Arc, Mutex};

struct Frozen;

impl LawClock for Frozen {
    fn now_s(&self) -> f64 {
        0.0
    }
}

struct Run {
    ticks: Vec<TickObservation>,
    end: RunState,
    trip: Option<String>,
    counts: (u64, u64, u64),
}

fn run(case: SineCheckCase, factory: Arc<dyn SineLawFactory>, params: &ServoBlendParams) -> Run {
    let mut cfg = sine_check_config(case, SINE_CHECK_FS_HZ);
    cfg.blend_w = params.blend_w;
    cfg.compression = params.compression;
    cfg.law_guards = Some(LawGuards {
        budget_fraction: 1.0,
        ..LawGuards::default()
    });
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let mut s = SineService::new(
        cfg,
        sine_check_table(case),
        station_for(case),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        SINE_CHECK_SEED,
        Some(std::env::temp_dir().join("hertzforge-sine-same-door")),
    )
    .unwrap()
    .with_law_factory(factory)
    .with_law_clock(Arc::new(Frozen))
    .with_tick_observer(Box::new(move |o: &TickObservation| {
        sink.lock().unwrap().push(*o)
    }));
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_sysid(0).unwrap();
    s.coherence_gate().unwrap();
    let mut n = 0;
    while matches!(s.state(), RunState::Engaged | RunState::FullLevel) && n < 2_000 {
        if !s.run_chunk().unwrap() {
            break;
        }
        n += 1;
    }
    let t = s.law_tally();
    let ticks = seen.lock().unwrap().clone();
    Run {
        ticks,
        end: s.state(),
        trip: s.trip().map(|t| format!("{:?}", t.kind)),
        counts: (t.calls, t.misses, t.capped),
    }
}

fn file_factory(params: &ServoBlendParams) -> Arc<dyn SineLawFactory> {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let bytes = std::fs::read(law_file("hertzforge_law_sine_servo")).unwrap();
    let law = host.load_sine(&bytes).unwrap();
    let compression = match params.compression {
        CompressionPreset::Slow => "slow",
        CompressionPreset::Fast => "fast",
        CompressionPreset::Custom { .. } => "custom",
    };
    let p = law
        .resolve_params(
            &[
                Param::new("blend_w", ParamValue::Number(params.blend_w)),
                Param::new("compression", ParamValue::Choice(compression.into())),
                Param::new("kp", ParamValue::Number(params.kp)),
            ],
            1,
        )
        .unwrap();
    Arc::new(WasmSineLawFactory::new(law, p))
}

/// The worst relative difference between the two runs' drives and values, and
/// whether every discrete fact agreed.
fn compare(a: &Run, b: &Run) -> (f64, bool) {
    let rel = |x: f64, y: f64| {
        if x == y {
            0.0
        } else {
            (x - y).abs() / x.abs().max(y.abs()).max(1e-300)
        }
    };
    let mut worst = 0.0f64;
    let mut same = a.ticks.len() == b.ticks.len()
        && a.end == b.end
        && a.trip == b.trip
        && a.counts == b.counts;
    for (x, y) in a.ticks.iter().zip(&b.ticks) {
        worst = worst
            .max(rel(x.drive, y.drive))
            .max(rel(x.servo_step_db, y.servo_step_db))
            .max(rel(x.servo_level_db, y.servo_level_db))
            .max(rel(x.measured, y.measured));
        same &= x.tick == y.tick && x.notch_binding == y.notch_binding && x.level_db == y.level_db;
    }
    (worst, same)
}

fn defaults() -> ServoBlendParams {
    ServoBlendParams::default()
}

fn fast_w0() -> ServoBlendParams {
    ServoBlendParams {
        blend_w: 0.0,
        compression: CompressionPreset::Fast,
        kp: 0.3,
    }
}

#[test]
fn the_built_in_and_its_law_file_run_the_same_at_both_parameter_sets() {
    let mut runs = Vec::new();
    for (name, p) in [("default", defaults()), ("w 0, fast", fast_w0())] {
        let native = run(
            SineCheckCase::Fast,
            Arc::new(ServoBlendFactory { params: p }),
            &p,
        );
        let file = run(SineCheckCase::Fast, file_factory(&p), &p);
        let (worst, same) = compare(&native, &file);
        println!(
            "{name}: {} ticks, end {:?}, worst relative difference {worst:.3e}, discrete same {same}",
            native.ticks.len(),
            native.end
        );
        assert!(native.ticks.len() > 1000, "{name}: the run ran");
        assert!(same, "{name}: a discrete fact differs");
        assert!(worst <= 1e-9, "{name}: {worst:e}");
        runs.push(native);
    }
    // The second parameter set moves the run.
    let moved = runs[0]
        .ticks
        .iter()
        .zip(&runs[1].ticks)
        .map(|(a, b)| (20.0 * (a.drive / b.drive).log10()).abs())
        .fold(0.0f64, f64::max);
    assert!(moved > 1e-3, "the parameters moved the drive {moved} dB");
}

#[test]
fn the_limit_channel_case_is_reported() {
    let p = defaults();
    let native = run(
        SineCheckCase::Notch,
        Arc::new(ServoBlendFactory { params: p }),
        &p,
    );
    let file = run(SineCheckCase::Notch, file_factory(&p), &p);
    let (worst, same) = compare(&native, &file);
    println!(
        "limit channel: {} ticks, worst relative difference {worst:.3e}, discrete same {same}",
        native.ticks.len()
    );
}
