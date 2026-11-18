//! The records: the law-call record replays to the same drives, the readings
//! for several shakers are formed every update, and the sweep-rate warning is
//! returned before arming.

mod common;

use common::*;
use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_mimo_sine::records::{replay, Outcome};
use hertzforge_mimo_sine::service::{EngineOptions, MimoSineService};
use hertzforge_mimo_sine::*;
use hertzforge_module_abi::StationClass;
use hertzforge_sine::scheduler::{BidirSweepConfig, SweepDirection, SweepTarget};
use hertzforge_sine::SweepRate;

fn sweep(rate: f64) -> TestPlan {
    TestPlan::Sweep(BidirSweepConfig {
        band: (30.0, 300.0),
        rate: SweepRate::LogOctPerMin(rate),
        start_hz: 30.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(1),
    })
}

#[test]
fn mimo_sine_the_law_call_record_replays_to_the_same_drives() {
    let s = SineStructure::Linear4x4;
    let f = law("matrix");
    let r = run(
        engaged(s, setup(s, sweep(6.0), "matrix"), f.clone(), NOISE),
        60.0,
    );
    assert_eq!(r.outcome, Some(Outcome::Completed));
    let ls = r.service.law_setup().unwrap().clone();
    let called = r.ticks.iter().filter(|t| t.law_called).count();
    assert!(called > 1000, "{called}");
    let bad = replay(f.as_ref(), &ls, &r.ticks, 1e-12).unwrap();
    assert!(
        bad.is_empty(),
        "{} of {called} replays differ: {:?}",
        bad.len(),
        &bad[..bad.len().min(3)]
    );
    // A record that was tampered with is caught.
    let mut t = r.ticks.clone();
    let k = t
        .iter()
        .position(|x| x.law_called && x.index > 500)
        .unwrap();
    t[k].control[0] *= 1.01;
    let bad = replay(f.as_ref(), &ls, &t, 1e-12).unwrap();
    assert!(!bad.is_empty());
    assert_eq!(r.service.records_dropped(), 0);
}

#[test]
fn mimo_sine_readings_are_formed_every_update() {
    for s in [SineStructure::Linear4x4, SineStructure::Wide3x6] {
        let r = run(
            engaged(
                s,
                setup(s, dwell(96.0, 4.0), "matrix"),
                law("matrix"),
                NOISE,
            ),
            8.0,
        );
        let m = s.dims().1 as f64;
        for t in r.ticks.iter().filter(|t| t.time_s > 1.0) {
            assert!(
                t.opposition_ratio.is_finite() && t.opposition_ratio >= 1.0 - 1e-6,
                "{}",
                t.opposition_ratio
            );
            assert!(t.largest_share >= 1.0 / m - 1e-12 && t.largest_share <= 1.0);
        }
        // `matrix` takes the least drive: nothing opposes.
        let t = r.ticks.iter().rev().find(|t| !t.settling).unwrap();
        assert!(
            t.opposition_ratio < 1.01,
            "{} {}",
            s.name(),
            t.opposition_ratio
        );
    }
}

#[test]
fn mimo_sine_the_sweep_rate_warning_flags_a_light_mode_swept_too_fast() {
    let s = SineStructure::Linear4x4;
    let warn = |rate: f64| {
        let mut svc = MimoSineService::new(
            setup(s, sweep(rate), "matrix"),
            table(s),
            backend(s, NOISE, 7),
            law("matrix"),
            // On the simulator the law's time budget is relaxed, as the other simulator
            // harnesses relax theirs, so a call a busy machine made late is not a miss here.
            EngineOptions {
                law_budget: Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET),
                ..EngineOptions::new(StationClass::HertzforgeSim)
            },
        )
        .unwrap();
        svc.arm().unwrap();
        svc.identify().unwrap().sweep_rate_warning.clone()
    };
    let slow = warn(1.0);
    let fast = warn(8.0);
    // The 40 Hz mode (Q 50): 4.4 of its time constants inside its bandwidth
    // at one octave a minute, 0.54 at eight.
    let at40 = |w: &[hertzforge_mimo_sine_law_math::readings::SweepRateReading]| {
        *w.iter()
            .find(|r| (r.f_hz - 40.0).abs() < 1.0)
            .expect("the 40 Hz mode is found")
    };
    assert!(!at40(&slow).flagged);
    assert!(at40(&fast).flagged);
    assert!((at40(&slow).time_constants / at40(&fast).time_constants - 8.0).abs() < 1e-6);
    assert!((at40(&slow).q - 50.0).abs() < 1.0);
}
