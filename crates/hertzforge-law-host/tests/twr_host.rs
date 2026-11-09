//! The law host's `twr-single` world: a TWR law file loads, is told apart from
//! every other world's, and each way the misbehaving TWR fixture goes wrong
//! comes back as a named [`LawError`], stopped within its budget.

use hertzforge_law_host::{
    AnyLaw, ClassicalPlant, HostConfig, LawError, LawHost, Param, ParamValue, TwrInputs,
    TwrLawInstance, TwrOutputRule, TwrPlayKind, TwrSetup, TwrVerdict,
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

const FIXTURE: &str = "law_fixture_twr_misbehave";
const CORRECT: &str = "hertzforge_law_twr_correct";
const TEMPLATE: &str = "law_fixture_twr_template";
const BUDGET: Duration = Duration::from_secs(5);
const N: usize = 256;

struct Grid {
    reference: Vec<f64>,
    frf: Vec<(f64, f64)>,
    coherence: Vec<f64>,
    trusted: Vec<bool>,
}

fn grid() -> Grid {
    let bins = N / 2 + 1;
    Grid {
        reference: (0..N)
            .map(|i| {
                if (64..192).contains(&i) {
                    (std::f64::consts::TAU * 8.0 * (i - 64) as f64 / 128.0).sin()
                } else {
                    0.0
                }
            })
            .collect(),
        frf: vec![(1.0, 0.0); bins],
        coherence: vec![1.0; bins],
        trusted: (0..bins).map(|k| k > 0 && k < bins - 1).collect(),
    }
}

fn setup(g: &Grid) -> TwrSetup<'_> {
    TwrSetup {
        fs_hz: 2560.0,
        record_len: N,
        band_hz: (5.0, 1000.0),
        reference: &g.reference,
        section_start: 64,
        section_len: 128,
        plant: ClassicalPlant {
            frf: &g.frf,
            coherence: &g.coherence,
            inverse: &g.frf,
            trusted: &g.trusted,
            epsilon: 0.0,
        },
        drive_ceiling: 2.0,
    }
}

fn instance(host: &LawHost, mode: &str) -> TwrLawInstance {
    let law = host.load_twr(&law_bytes(FIXTURE)).unwrap();
    let g = grid();
    law.instantiate(
        &setup(&g),
        &[Param::new("mode", ParamValue::Choice(mode.into()))],
        BUDGET,
    )
    .unwrap()
}

fn after(response: &[f64]) -> TwrInputs<'_> {
    TwrInputs {
        play_index: 1,
        kind: TwrPlayKind::Iteration,
        response,
        drive: response,
        judged: Some(TwrVerdict {
            gated: false,
            rms_error_pct: 10.0,
            peak_error_pct: 12.0,
            spectral_error_db: 0.9,
        }),
        limited: false,
    }
}

const SEED: TwrInputs<'static> = TwrInputs {
    play_index: 0,
    kind: TwrPlayKind::Iteration,
    response: &[],
    drive: &[],
    judged: None,
    limited: false,
};

/// Seed, then one correction against a response of 0.9 of the reference.
fn iterate_once(
    mut law: TwrLawInstance,
    budget: Duration,
) -> (TwrLawInstance, Result<Vec<f64>, LawError>, Duration) {
    let seed = law
        .seed(&SEED, BUDGET)
        .expect("the seed is the built-in law's");
    assert_eq!(seed.drive.len(), N);
    let response: Vec<f64> = grid().reference.iter().map(|v| 0.9 * v).collect();
    let t0 = Instant::now();
    let r = law.iterate(&after(&response), budget).map(|o| o.drive);
    (law, r, t0.elapsed())
}

#[test]
fn a_twr_law_file_loads_and_is_told_apart_from_every_other_world() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    for (name, law_name) in [
        (CORRECT, "frequency-domain correction"),
        (TEMPLATE, "plain correction"),
        (FIXTURE, "misbehaving TWR fixture"),
    ] {
        let any = host.load_any(&law_bytes(name)).unwrap();
        assert!(matches!(any, AnyLaw::Twr(_)), "{name}");
        assert_eq!(any.world(), "twr-single");
        assert_eq!(any.identity().name, law_name);
    }
    let bytes = law_bytes(CORRECT);
    assert!(matches!(host.load(&bytes), Err(LawError::NotALaw(_))));
    assert!(matches!(
        host.load_classical(&bytes),
        Err(LawError::NotALaw(_))
    ));
    for (other, words) in [
        ("hertzforge_law_ratio", "Random law"),
        ("hertzforge_law_shock_fdc", "shock law"),
        ("hertzforge_law_sine_servo", "Sine law"),
    ] {
        match host.load_twr(&law_bytes(other)) {
            Err(LawError::NotALaw(m)) => assert!(m.contains(words), "{other}: {m}"),
            r => panic!("{other} loaded as a TWR law: {:?}", r.is_ok()),
        }
    }
}

#[test]
fn the_law_file_seeds_the_reference_through_its_inverse() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = host.load_twr(&law_bytes(CORRECT)).unwrap();
    let keys: Vec<&str> = law
        .manifest()
        .params
        .iter()
        .map(|p| p.key.as_str())
        .collect();
    assert_eq!(keys, ["beta", "max_db_per_iter", "initial_backoff_db"]);
    let g = grid();
    let mut inst = law.instantiate(&setup(&g), &[], BUDGET).unwrap();
    let out = inst.seed(&SEED, BUDGET).unwrap();
    // A unity plant and inverse on every bin but DC and Nyquist: the seed is
    // the reference, less its DC.
    let mean = g.reference.iter().sum::<f64>() / N as f64;
    let worst = out
        .drive
        .iter()
        .zip(&g.reference)
        .map(|(d, r)| (d - (r - mean)).abs())
        .fold(0.0f64, f64::max);
    assert!(worst < 1e-9, "{worst}");
    let keys: Vec<&str> = out.values.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(keys, ["corrected_bins", "clamped_bins", "drive_change_db"]);
    // A parameter out of its declared range refuses the build.
    assert!(matches!(
        law.instantiate(
            &setup(&g),
            &[Param::new("beta", ParamValue::Number(2.0))],
            BUDGET
        ),
        Err(LawError::ParamInvalid { .. })
    ));
}

#[test]
fn each_twr_misbehaviour_is_a_named_error() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let rule = |r: &Result<Vec<f64>, LawError>| match r {
        Err(LawError::BadTwrOutput { rule, .. }) => Some(*rule),
        _ => None,
    };
    let (_, r, _) = iterate_once(instance(&host, "ok"), BUDGET);
    assert_eq!(r.unwrap().len(), N);
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
        Some(TwrOutputRule::NonFinite)
    );
    assert_eq!(
        rule(&iterate_once(instance(&host, "wrong_length"), BUDGET).1),
        Some(TwrOutputRule::WrongLength)
    );
    assert_eq!(
        rule(&iterate_once(instance(&host, "undeclared"), BUDGET).1),
        Some(TwrOutputRule::UndeclaredValue)
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
fn a_late_twr_law_is_stopped_at_its_budget_and_stays_stopped() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let budget = Duration::from_millis(50);
    for mode in ["spin", "sleep_after"] {
        let mut law = instance(&host, mode);
        law.seed(&SEED, BUDGET).unwrap();
        let response: Vec<f64> = grid().reference.iter().map(|v| 0.9 * v).collect();
        if mode == "sleep_after" {
            law.iterate(&after(&response), budget)
                .expect("the first correction answers");
        }
        let t0 = Instant::now();
        let r = law.iterate(&after(&response), budget);
        let took = t0.elapsed();
        assert_eq!(r, Err(LawError::Deadline { budget }), "{mode}");
        assert!(
            took >= budget && took < budget + Duration::from_millis(100),
            "{mode}: {took:?}"
        );
        assert!(matches!(
            law.iterate(&after(&response), budget),
            Err(LawError::Poisoned(_))
        ));
    }
}

/// Timing list: the deadline lands within a tick or so of the budget.
#[test]
fn a_spinning_twr_law_is_stopped_within_a_tick_of_its_budget() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let budget = Duration::from_millis(50);
    let mut worst = Duration::ZERO;
    for _ in 0..5 {
        let (_, r, took) = iterate_once(instance(&host, "spin"), budget);
        assert_eq!(r, Err(LawError::Deadline { budget }));
        assert!(took >= budget, "stopped early, after {took:?}");
        worst = worst.max(took - budget);
    }
    println!("TWR deadline overshoot, worst of 5 at a 50 ms budget: {worst:?}");
    assert!(worst < Duration::from_millis(100), "overshoot {worst:?}");
}
