//! The law host's `shock-srs` world: an SRS law file loads, is told apart
//! from a classical and a Random one, and each way the misbehaving SRS
//! fixture goes wrong comes back as a named [`LawError`], stopped within its
//! budget.

use hertzforge_law_host::{
    AnyLaw, HostConfig, LawError, LawHost, Param, ParamValue, SrsInputs, SrsLawInstance,
    SrsOutputRule, SrsSetup,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn law_bytes(name: &str) -> Vec<u8> {
    let file = format!("{name}.wasm");
    let exe = std::env::current_exe().expect("the test binary knows its own path");
    for dir in exe.ancestors().skip(1).take(4) {
        let p: PathBuf = dir.join("wasm32-wasip2").join("release").join(&file);
        if p.is_file() {
            return std::fs::read(&p).unwrap();
        }
    }
    panic!("the law file {file} is not built. Run `bash scripts/build_laws.sh`.");
}

const FIXTURE: &str = "law_fixture_srs_misbehave";
const RATIO: &str = "hertzforge_law_shock_srs_ratio";
const BUDGET: Duration = Duration::from_secs(5);

/// A 1/3-octave grid from 100 to 2000 Hz with one component per band.
struct Grid {
    bands: Vec<f64>,
    target: Vec<f64>,
    gain: Vec<f64>,
}

fn grid() -> Grid {
    let bands: Vec<f64> = (0..14).map(|i| 100.0 * 2f64.powf(i as f64 / 3.0)).collect();
    Grid {
        target: vec![10.0; bands.len()],
        gain: vec![2.0; bands.len()],
        bands,
    }
}

fn setup(g: &Grid) -> SrsSetup<'_> {
    SrsSetup {
        band_hz: &g.bands,
        component_hz: &g.bands,
        target: &g.target,
        q: 10.0,
        component_gain: Some(&g.gain),
    }
}

fn instance(host: &LawHost, mode: &str) -> SrsLawInstance {
    let law = host.load_srs(&law_bytes(FIXTURE)).unwrap();
    let g = grid();
    law.instantiate(
        &setup(&g),
        &[Param::new("mode", ParamValue::Choice(mode.into()))],
        BUDGET,
    )
    .unwrap()
}

fn inputs<'a>(target: &'a [f64], measured: &'a [f64], pulse_index: u64) -> SrsInputs<'a> {
    SrsInputs {
        pulse_index,
        target,
        measured,
        commanded_scale: 1.0,
    }
}

/// Seed, then one iteration against a response 1 dB low in every band.
fn iterate_once(
    mut law: SrsLawInstance,
    budget: Duration,
) -> (SrsLawInstance, Result<Vec<f64>, LawError>, Duration) {
    let g = grid();
    let zeros = vec![0.0; g.bands.len()];
    let seed = law
        .seed(&inputs(&g.target, &zeros, 0), BUDGET)
        .expect("the seed is the built-in law's");
    assert_eq!(seed.gains, vec![0.5; g.bands.len()], "1 / |H|");
    let measured: Vec<f64> = g
        .target
        .iter()
        .map(|t| t * 10f64.powf(-1.0 / 20.0))
        .collect();
    let t0 = Instant::now();
    let r = law
        .iterate(&inputs(&g.target, &measured, 1), budget)
        .map(|o| o.gains);
    (law, r, t0.elapsed())
}

#[test]
fn an_srs_law_file_loads_and_is_told_apart_from_the_other_worlds() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let bytes = law_bytes(RATIO);
    let any = host.load_any(&bytes).unwrap();
    assert!(matches!(any, AnyLaw::ShockSrs(_)));
    assert_eq!(any.world(), "shock-srs");
    assert_eq!(any.identity().name, "SRS ratio");
    assert!(matches!(
        host.load_classical(&bytes),
        Err(LawError::NotALaw(_))
    ));
    match host.load_srs(&law_bytes("hertzforge_law_shock_fdc")) {
        Err(LawError::NotALaw(m)) => assert!(m.contains("classical shock law"), "{m}"),
        r => panic!("a classical law loaded as an SRS one: {:?}", r.is_ok()),
    }
    match host.load_srs(&law_bytes("hertzforge_law_ratio")) {
        Err(LawError::NotALaw(m)) => assert!(m.contains("Random law"), "{m}"),
        r => panic!("a Random law loaded as an SRS one: {:?}", r.is_ok()),
    }
}

#[test]
fn the_law_file_seeds_from_the_identified_gain_and_follows_the_ratio() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = host.load_srs(&law_bytes(RATIO)).unwrap();
    let g = grid();
    let mut inst = law.instantiate(&setup(&g), &[], BUDGET).unwrap();
    let zeros = vec![0.0; g.bands.len()];
    let seed = inst.seed(&inputs(&g.target, &zeros, 0), BUDGET).unwrap();
    assert_eq!(seed.gains, vec![0.5; g.bands.len()]);
    let keys: Vec<&str> = seed.values.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(
        keys,
        ["worst_band_error_db", "worst_band_hz", "clamped_components"]
    );
    let measured: Vec<f64> = g
        .target
        .iter()
        .map(|t| t * 10f64.powf(-1.0 / 20.0))
        .collect();
    let out = inst
        .iterate(&inputs(&g.target, &measured, 1), BUDGET)
        .unwrap();
    for gain in out.gains {
        assert!((gain - 10f64.powf(1.0 / 20.0)).abs() < 1e-12);
    }
}

#[test]
fn each_misbehaviour_is_a_named_error() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let rule = |r: &Result<Vec<f64>, LawError>| match r {
        Err(LawError::BadSrsOutput { rule, .. }) => Some(*rule),
        _ => None,
    };
    let (_, r, _) = iterate_once(instance(&host, "ok"), BUDGET);
    assert_eq!(r.unwrap().len(), grid().bands.len());
    let (law, r, _) = iterate_once(instance(&host, "trap"), BUDGET);
    assert!(
        matches!(&r, Err(LawError::Trap(m)) if m.contains("deliberate trap")),
        "{r:?}"
    );
    assert!(law.is_poisoned());
    let (law, r, _) = iterate_once(instance(&host, "grow"), BUDGET);
    assert!(matches!(r, Err(LawError::Memory { .. })), "{r:?}");
    assert!(law.is_poisoned());
    let (_, r, took) = iterate_once(instance(&host, "sleep"), BUDGET);
    assert!(
        r.is_ok() && took < Duration::from_secs(1),
        "a sleep returns at once: {took:?}"
    );
    let (_, r, _) = iterate_once(instance(&host, "open_file"), BUDGET);
    assert!(
        matches!(&r, Err(LawError::Law(m)) if m.contains("every escape failed")),
        "{r:?}"
    );
    assert_eq!(
        rule(&iterate_once(instance(&host, "nan"), BUDGET).1),
        Some(SrsOutputRule::NonFinite)
    );
    assert_eq!(
        rule(&iterate_once(instance(&host, "zero"), BUDGET).1),
        Some(SrsOutputRule::NotPositive)
    );
    assert_eq!(
        rule(&iterate_once(instance(&host, "short"), BUDGET).1),
        Some(SrsOutputRule::WrongLength)
    );
    assert_eq!(
        rule(&iterate_once(instance(&host, "undeclared"), BUDGET).1),
        Some(SrsOutputRule::UndeclaredValue)
    );
    let (law, r, _) = iterate_once(instance(&host, "law_error"), BUDGET);
    assert!(matches!(r, Err(LawError::Law(_))));
    assert!(!law.is_poisoned(), "the law's own error leaves it usable");
    let (law, r, _) = iterate_once(instance(&host, "print"), BUDGET);
    assert!(r.is_ok());
    let out = law.guest_output();
    assert!(
        !out.stdout.is_empty() && out.stdout_dropped > 0,
        "{:?}",
        out.stdout_dropped
    );
}

#[test]
fn a_late_srs_law_is_stopped_at_its_budget_and_stays_stopped() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let budget = Duration::from_millis(50);
    let g = grid();
    let zeros = vec![0.0; g.bands.len()];
    for mode in ["spin", "sleep_after"] {
        let mut law = instance(&host, mode);
        law.seed(&inputs(&g.target, &zeros, 0), BUDGET).unwrap();
        if mode == "sleep_after" {
            law.iterate(&inputs(&g.target, &g.target, 1), budget)
                .expect("the first iteration answers");
        }
        let t0 = Instant::now();
        let r = law.iterate(&inputs(&g.target, &g.target, 2), budget);
        let took = t0.elapsed();
        assert_eq!(r, Err(LawError::Deadline { budget }), "{mode}");
        assert!(
            took >= budget && took < budget + Duration::from_millis(100),
            "{mode}: {took:?}"
        );
        assert!(matches!(
            law.iterate(&inputs(&g.target, &g.target, 3), budget),
            Err(LawError::Poisoned(_))
        ));
    }
}

/// Timing list: the deadline lands within a tick or so of the budget.
#[test]
fn a_spinning_srs_law_is_stopped_within_a_tick_of_its_budget() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let budget = Duration::from_millis(50);
    let mut worst = Duration::ZERO;
    for _ in 0..5 {
        let (_, r, took) = iterate_once(instance(&host, "spin"), budget);
        assert_eq!(r, Err(LawError::Deadline { budget }));
        assert!(took >= budget, "stopped early, after {took:?}");
        worst = worst.max(took - budget);
    }
    println!("SRS deadline overshoot, worst of 5 at a 50 ms budget: {worst:?}");
    assert!(worst < Duration::from_millis(100), "overshoot {worst:?}");
}
