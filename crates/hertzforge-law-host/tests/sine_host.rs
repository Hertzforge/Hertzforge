//! The law host's `sine-single` world: a Sine law file loads, is told apart
//! from the other worlds, reports its live parameters, and each way the
//! misbehaving Sine fixture goes wrong comes back as a named [`LawError`],
//! a late one stopped within its budget plus two epoch ticks (1 ms each).

use hertzforge_law_host::{
    AnyLaw, HostConfig, LawError, LawHost, Param, ParamValue, SineHold, SineIdentification,
    SineInputs, SineLawInstance, SineOutputRule, SineSetup,
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

const FIXTURE: &str = "law_fixture_sine_misbehave";
const SERVO: &str = "hertzforge_law_sine_servo";
const TEMPLATE: &str = "law_fixture_sine_template";
const BUDGET: Duration = Duration::from_secs(5);

/// The core's budget at the default 10 ms tick: a quarter of it.
const TICK_BUDGET: Duration = Duration::from_micros(2_500);

struct Ident {
    f: Vec<f64>,
    h: Vec<f64>,
    c: Vec<f64>,
    t: Vec<bool>,
    target: Vec<f64>,
}

fn ident() -> Ident {
    let f: Vec<f64> = (1..=400).map(|k| k as f64 * 2.0).collect();
    Ident {
        h: vec![0.5; f.len()],
        c: vec![0.99; f.len()],
        t: vec![true; f.len()],
        target: vec![1.0; f.len()],
        f,
    }
}

fn setup(i: &Ident) -> SineSetup<'_> {
    SineSetup {
        control_channels: 1,
        tick_s: 0.01,
        band_hz: (100.0, 500.0),
        drive_ceiling: 5.0,
        identification: SineIdentification {
            frequency_hz: &i.f,
            gain: &i.h,
            coherence: &i.c,
            trusted: &i.t,
            target: &i.target,
        },
    }
}

const AMP: [f64; 1] = [0.9];
const PHASE: [f64; 1] = [-10.0];

fn inputs(tick_index: u64, measured: f64, drive: f64) -> SineInputs<'static> {
    SineInputs {
        tick_index,
        dt_s: 0.01,
        frequency_hz: 200.0,
        sweep_rate_hz_per_s: 0.0,
        level_db: 0.0,
        target: 1.0,
        measured,
        channel_amplitude: &AMP,
        channel_phase_deg: &PHASE,
        drive,
        prediction_db: Some(20.0 * 2f64.log10()),
        hold: SineHold::Free,
        notch_ceiling: None,
        settling: false,
    }
}

fn instance(host: &LawHost, mode: &str, after: i64) -> SineLawInstance {
    let law = host.load_sine(&law_bytes(FIXTURE)).unwrap();
    let i = ident();
    law.instantiate(
        &setup(&i),
        &[
            Param::new("mode", ParamValue::Choice(mode.into())),
            Param::new("after", ParamValue::Integer(after)),
        ],
        BUDGET,
    )
    .unwrap()
}

/// Engage, then one tick against a response 1 dB low.
fn tick_once(
    mut law: SineLawInstance,
    budget: Duration,
) -> (SineLawInstance, Result<f64, LawError>, Duration) {
    let e = law
        .engage(&inputs(0, 0.0, 0.0), BUDGET)
        .expect("the engage is the built-in law's");
    assert!((e.drive - 2.0).abs() < 1e-12, "the prediction: {}", e.drive);
    let t0 = Instant::now();
    let r = law
        .tick(&inputs(1, 10f64.powf(-1.0 / 20.0), e.drive), budget)
        .map(|o| o.drive);
    (law, r, t0.elapsed())
}

#[test]
fn a_sine_law_file_loads_and_is_told_apart_from_the_other_worlds() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let bytes = law_bytes(SERVO);
    let any = host.load_any(&bytes).unwrap();
    assert!(matches!(any, AnyLaw::Sine(_)));
    assert_eq!(any.world(), "sine-single");
    assert_eq!(any.identity().name, "Servo blend");
    let law = host.load_sine(&bytes).unwrap();
    assert_eq!(law.live(), ["blend_w".to_string()]);
    let t = host.load_sine(&law_bytes(TEMPLATE)).unwrap();
    assert_eq!(t.live(), ["ki".to_string()]);
    assert!(matches!(host.load(&bytes), Err(LawError::NotALaw(_))));
    assert!(matches!(host.load_srs(&bytes), Err(LawError::NotALaw(_))));
    match host.load_sine(&law_bytes("hertzforge_law_shock_srs_ratio")) {
        Err(LawError::NotALaw(m)) => assert!(m.contains("shock law"), "{m}"),
        r => panic!("an SRS law loaded as a Sine one: {:?}", r.is_ok()),
    }
    match host.load_sine(&law_bytes("hertzforge_law_ratio")) {
        Err(LawError::NotALaw(m)) => assert!(m.contains("Random law"), "{m}"),
        r => panic!("a Random law loaded as a Sine one: {:?}", r.is_ok()),
    }
}

#[test]
fn the_servo_law_file_engages_on_the_prediction_and_moves_by_its_blend() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = host.load_sine(&law_bytes(SERVO)).unwrap();
    let i = ident();
    let mut inst = law.instantiate(&setup(&i), &[], BUDGET).unwrap();
    let e = inst.engage(&inputs(0, 0.0, 0.0), BUDGET).unwrap();
    assert!((e.drive - 2.0).abs() < 1e-12);
    let keys: Vec<&str> = e.values.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "servo_step_db",
            "prediction_step_db",
            "total_step_db",
            "servo_level_db"
        ]
    );
    let o = inst
        .tick(&inputs(1, 10f64.powf(-1.0 / 20.0), e.drive), BUDGET)
        .unwrap();
    assert!(o.drive > e.drive, "1 dB low: the drive rises");
    inst.adjust(&Param::new("blend_w", ParamValue::Number(0.2)), BUDGET)
        .unwrap();
    match inst.adjust(&Param::new("kp", ParamValue::Number(0.2)), BUDGET) {
        Err(LawError::ParamInvalid { key, .. }) => assert_eq!(key, "kp"),
        r => panic!("{r:?}"),
    }
    match inst.adjust(&Param::new("blend_w", ParamValue::Number(2.0)), BUDGET) {
        Err(LawError::ParamInvalid { key, .. }) => assert_eq!(key, "blend_w"),
        r => panic!("{r:?}"),
    }
}

#[test]
fn each_misbehaviour_is_a_named_error() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let rule = |r: &Result<f64, LawError>| match r {
        Err(LawError::BadSineOutput { rule, .. }) => Some(*rule),
        _ => None,
    };
    let (_, r, _) = tick_once(instance(&host, "ok", 1), BUDGET);
    assert!(r.unwrap() > 2.0);
    let (law, r, _) = tick_once(instance(&host, "trap", 1), BUDGET);
    assert!(
        matches!(&r, Err(LawError::Trap(m)) if m.contains("deliberate trap")),
        "{r:?}"
    );
    assert!(law.is_poisoned());
    let (law, r, _) = tick_once(instance(&host, "grow", 1), BUDGET);
    assert!(matches!(r, Err(LawError::Memory { .. })), "{r:?}");
    assert!(law.is_poisoned());
    let (_, r, took) = tick_once(instance(&host, "sleep", 1), BUDGET);
    assert!(
        r.is_ok() && took < Duration::from_secs(1),
        "a sleep returns at once: {took:?}"
    );
    let (_, r, _) = tick_once(instance(&host, "open_file", 1), BUDGET);
    assert!(
        matches!(&r, Err(LawError::Law(m)) if m.contains("every escape failed")),
        "{r:?}"
    );
    assert_eq!(
        rule(&tick_once(instance(&host, "nan", 1), BUDGET).1),
        Some(SineOutputRule::NonFinite)
    );
    assert_eq!(
        rule(&tick_once(instance(&host, "negative", 1), BUDGET).1),
        Some(SineOutputRule::Negative)
    );
    assert_eq!(
        rule(&tick_once(instance(&host, "undeclared", 1), BUDGET).1),
        Some(SineOutputRule::UndeclaredValue)
    );
    let (law, r, _) = tick_once(instance(&host, "law_error", 1), BUDGET);
    assert!(matches!(r, Err(LawError::Law(_))));
    assert!(!law.is_poisoned(), "the law's own error leaves it usable");
    let (law, r, _) = tick_once(instance(&host, "print", 1), BUDGET);
    assert!(r.is_ok());
    let out = law.guest_output();
    assert!(
        !out.stdout.is_empty() && out.stdout_dropped > 0,
        "{:?}",
        out.stdout_dropped
    );
    let mut law = instance(&host, "adjust_error", 1);
    match law.adjust(&Param::new("blend_w", ParamValue::Number(0.2)), BUDGET) {
        Err(LawError::Law(m)) => assert!(m.contains("deliberate refusal"), "{m}"),
        r => panic!("{r:?}"),
    }
    assert!(!law.is_poisoned(), "a refused change leaves the law usable");
}

/// At the core's own budget for a 10 ms tick, a spinning law and a law that
/// turns late are stopped within the budget plus about one epoch tick.
#[test]
fn a_late_sine_law_is_stopped_within_its_tick_budget_and_stays_stopped() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    for mode in ["spin", "sleep_after"] {
        let mut law = instance(&host, mode, 1);
        let e = law.engage(&inputs(0, 0.0, 0.0), BUDGET).unwrap();
        if mode == "sleep_after" {
            law.tick(&inputs(1, 1.0, e.drive), TICK_BUDGET)
                .expect("the first tick answers");
        }
        let t0 = Instant::now();
        let r = law.tick(&inputs(2, 1.0, e.drive), TICK_BUDGET);
        let took = t0.elapsed();
        assert_eq!(
            r,
            Err(LawError::Deadline {
                budget: TICK_BUDGET
            }),
            "{mode}"
        );
        assert!(
            took >= TICK_BUDGET && took < TICK_BUDGET + Duration::from_millis(5),
            "{mode}: {took:?}"
        );
        assert!(matches!(
            law.tick(&inputs(3, 1.0, e.drive), TICK_BUDGET),
            Err(LawError::Poisoned(_))
        ));
    }
}

/// Timing list: at the 2.5 ms budget the core gives a call at a 10 ms tick, a
/// spinning law is stopped within two 1 ms epoch ticks of the budget (worst of
/// 20), so a late law ends by 4.5 ms, inside its tick. The deadline is real
/// time checked once an epoch tick, so one tick is the mechanism's own
/// overshoot; the second is the ticker thread's lateness and the trap's unwind.
/// A machine under other load can stretch this; a failure here is recorded,
/// not loosened.
#[test]
fn a_spinning_sine_law_is_stopped_within_two_epoch_ticks_of_its_budget() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let mut worst = Duration::ZERO;
    for _ in 0..20 {
        let (_, r, took) = tick_once(instance(&host, "spin", 1), TICK_BUDGET);
        assert_eq!(
            r,
            Err(LawError::Deadline {
                budget: TICK_BUDGET
            })
        );
        assert!(took >= TICK_BUDGET, "stopped early, after {took:?}");
        worst = worst.max(took - TICK_BUDGET);
    }
    println!("Sine deadline overshoot, worst of 20 at a 2.5 ms budget: {worst:?}");
    assert!(
        worst <= 2 * HostConfig::default().epoch_tick,
        "overshoot {worst:?}"
    );
}
