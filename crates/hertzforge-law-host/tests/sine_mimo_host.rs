//! The law host's `sine-mimo` world: a MIMO Sine law file loads, is told
//! apart from the other worlds, runs contained, and every way it can
//! misbehave is reported by name.

use hertzforge_law_host::sine_mimo::{
    Complex, Hold, Identification, MatrixStack, ProportionalTracking, SineMimoInputs,
    SineMimoSetup, TestType, Tolerance, Tracking,
};
use hertzforge_law_host::{
    AnyLaw, HostConfig, LawError, LawHost, Param, ParamValue, SineMimoLawInstance,
    SineMimoOutputRule, SINE_MIMO_WORLD,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn law_bytes(name: &str) -> Vec<u8> {
    let file = format!("{name}.wasm");
    let exe = std::env::current_exe().expect("the test binary knows its own path");
    for dir in exe.ancestors().skip(1) {
        let p: PathBuf = dir.join("wasm32-wasip2").join("release").join(&file);
        if p.is_file() {
            return std::fs::read(&p).unwrap();
        }
    }
    panic!("the law file {file} is not built. Run `bash scripts/build_laws.sh`.");
}

fn refusal(r: Result<hertzforge_law_host::LoadedSineMimoLaw, LawError>) -> String {
    match r {
        Err(LawError::NotALaw(m)) => m,
        Err(e) => panic!("refused, but not as a law of another world: {e}"),
        Ok(_) => panic!("a law of another world loaded as a MIMO Sine law"),
    }
}

const BUDGET: Duration = Duration::from_secs(5);
const MISBEHAVE: &str = "law_fixture_sine_mimo_misbehave";
const TEMPLATE: &str = "law_fixture_sine_mimo_template";

/// Two controls, three drives, one limit channel.
fn setup(test_type: TestType) -> SineMimoSetup {
    sized(2, 3, 1, test_type)
}

/// `nc` controls, `nd` drives and `nl` limit channels.
fn sized(nc: u32, nd: u32, nl: u32, test_type: TestType) -> SineMimoSetup {
    let grid = vec![20.0, 60.0, 100.0, 140.0, 200.0];
    let lines = grid.len() as u32;
    let stack = |rows: u32| MatrixStack {
        lines,
        rows,
        cols: nd,
        data: (0..lines * rows * nd)
            .map(|i| Complex {
                re: 1.0 + f64::from(i % 4),
                im: 0.25 * f64::from(i % 3),
            })
            .collect(),
    };
    let per = |n: u32, v: f64| vec![v; (n * lines) as usize];
    SineMimoSetup {
        controls: nc,
        drives: nd,
        limits: nl,
        fs_hz: 10240.0,
        tick_s: 0.01,
        band_lo_hz: 20.0,
        band_hi_hz: 200.0,
        test_type,
        shape: hertzforge_law_host::sine_mimo::shape_of(nc, nd),
        drive_ceiling_v: (1..=nd).map(f64::from).collect(),
        identification: Identification {
            frequency_hz: grid,
            response: stack(nc),
            limit_response: stack(nl),
            coherence: per(nc, 0.95),
            limit_coherence: per(nl, 0.95),
            trusted: vec![true; lines as usize],
        },
        identification_level_db: -20.0,
        reference: vec![Complex { re: 1.0, im: 0.0 }; (nc * lines) as usize],
        phase_specified: true,
        limit_bounds: vec![Some(10.0); (nl * lines) as usize],
        tolerance: Tolerance {
            alarm_low_db: per(nc, 3.0),
            alarm_high_db: per(nc, 3.0),
            abort_low_db: per(nc, 6.0),
            abort_high_db: per(nc, 6.0),
        },
        tracking: Tracking::Proportional(ProportionalTracking {
            share: 0.1,
            max_hz: 20.0,
        }),
        tracker_delay_factor: 1.0 / std::f64::consts::PI,
        transport_delay_s: 0.0,
    }
}

fn inputs(s: &SineMimoSetup, index: u64) -> SineMimoInputs {
    let c = |re: f64| Complex { re, im: 0.0 };
    SineMimoInputs {
        index,
        dt_s: if index == 0 { 0.0 } else { 0.01 },
        frequency_hz: 100.0,
        sweep_hz_per_s: 0.0,
        sweep_oct_per_min: 0.0,
        level: 1.0,
        reference: vec![c(1.0); s.controls as usize],
        controls: vec![c(0.5); s.controls as usize],
        limits: vec![c(0.2); s.limits as usize],
        limit_bounds: vec![Some(10.0); s.limits as usize],
        drive_in_force: vec![c(if index == 0 { 0.0 } else { 0.1 }); s.drives as usize],
        hold: if index == 0 { Hold::Engage } else { Hold::Free },
        notch_scale: 1.0,
        settling: index == 0,
    }
}

fn misbehave(host: &LawHost, mode: &str) -> SineMimoLawInstance {
    let law = host.load_sine_mimo(&law_bytes(MISBEHAVE)).unwrap();
    law.instantiate(
        &setup(TestType::Sweep),
        &[Param {
            key: "mode".into(),
            value: ParamValue::Choice(mode.into()),
        }],
        BUDGET,
    )
    .unwrap()
}

#[test]
fn mimo_sine_the_world_is_named_sine_mimo() {
    assert_eq!(SINE_MIMO_WORLD, "sine-mimo");
}

#[test]
fn mimo_sine_a_law_of_another_world_is_refused_by_name() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let m = refusal(host.load_sine_mimo(&law_bytes("hertzforge_law_mimo_shock_paired")));
    assert!(m.contains("MIMO Shock law (the shock-mimo world)"), "{m}");
    assert!(m.contains("not a MIMO Sine law"), "{m}");
    let m = refusal(host.load_sine_mimo(&law_bytes("hertzforge_law_sine_servo")));
    assert!(
        m.contains("single-output Sine law (the sine-single world)"),
        "{m}"
    );
    let m = refusal(host.load_sine_mimo(&law_bytes("hertzforge_law_mimo_pinv")));
    assert!(m.contains("multiple-output Random law"), "{m}");
    let m = refusal(host.load_sine_mimo(&law_bytes("hertzforge_law_shock_srs_ratio")));
    assert!(m.contains("another test type"), "{m}");
}

#[test]
fn mimo_sine_a_law_of_another_world_still_loads_as_its_own() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let any = host
        .load_any(&law_bytes("hertzforge_law_mimo_shock_paired"))
        .unwrap();
    assert!(matches!(any, AnyLaw::ShockMimo(_)));
    assert_ne!(any.world(), SINE_MIMO_WORLD);
}

#[test]
fn mimo_sine_the_fixtures_load_as_their_world_and_state_what_they_accept() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    for (name, types) in [
        (
            MISBEHAVE,
            vec![TestType::Sweep, TestType::Dwell, TestType::Stepped],
        ),
        (TEMPLATE, vec![TestType::Dwell, TestType::Stepped]),
    ] {
        match host.load_any(&law_bytes(name)) {
            Ok(AnyLaw::SineMimo(l)) => {
                assert_eq!(l.manifest().test_types, types, "{name}");
                assert_eq!(l.manifest().shapes.len(), 3, "{name}");
                assert!(l.manifest().amplitude_only, "{name}");
                assert_eq!(l.identity().sha256.len(), 64);
            }
            Ok(other) => panic!("{name} loaded as {}", other.world()),
            Err(e) => panic!("{name}: {e}"),
        }
    }
}

#[test]
fn mimo_sine_the_template_runs_contained_and_answers_the_run_shape() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = host.load_sine_mimo(&law_bytes(TEMPLATE)).unwrap();
    let s = setup(TestType::Dwell);
    let mut i = law.instantiate(&s, &[], BUDGET).unwrap();
    // Every declared parameter is completed with its default.
    assert_eq!(i.params().len(), 2);
    let first = i.engage(&inputs(&s, 0), BUDGET).unwrap();
    assert_eq!(first.drive.len(), 3);
    // The third drive has no control: silent.
    assert_eq!((first.drive[2].re, first.drive[2].im), (0.0, 0.0));
    let next = i.tick(&inputs(&s, 1), BUDGET).unwrap();
    assert_eq!(next.drive.len(), 3);
    assert_eq!(next.values[0].key, "error_db");
    // Half the wanted amplitude at gain 0.2 asks for 1.2 dB; the largest step
    // (1 dB by default) takes 1 dB.
    let up = 10f64.powf(1.0 / 20.0);
    assert!((next.drive[0].re - 0.1 * up).abs() < 1e-12);
}

#[test]
fn mimo_sine_a_run_the_law_does_not_accept_is_refused_before_it_is_built() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let template = host.load_sine_mimo(&law_bytes(TEMPLATE)).unwrap();
    match template.instantiate(&setup(TestType::Sweep), &[], BUDGET) {
        Err(LawError::SetupInvalid(m)) => {
            assert!(m.contains("accepts dwell and stepped, not sweep"), "{m}")
        }
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("the template was built for a sweep"),
    }
    // A bad setup never reaches the law.
    let mut s = setup(TestType::Dwell);
    s.drive_ceiling_v.pop();
    assert!(matches!(
        template.instantiate(&s, &[], BUDGET),
        Err(LawError::SetupInvalid(_))
    ));
    // A parameter the law does not declare.
    assert!(matches!(
        template.instantiate(
            &setup(TestType::Dwell),
            &[Param {
                key: "nope".into(),
                value: ParamValue::Number(1.0)
            }],
            BUDGET
        ),
        Err(LawError::ParamInvalid { .. })
    ));
}

#[test]
fn mimo_sine_each_misbehaviour_is_reported_by_name() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let s = setup(TestType::Sweep);
    let rule = |mode: &str| -> Option<SineMimoOutputRule> {
        let mut law = misbehave(&host, mode);
        law.engage(&inputs(&s, 0), BUDGET).unwrap();
        let r = match law.tick(&inputs(&s, 1), BUDGET) {
            Err(LawError::BadSineMimoOutput { rule, detail }) => {
                assert!(!detail.is_empty());
                Some(rule)
            }
            Ok(_) => None,
            Err(e) => panic!("{mode}: {e}"),
        };
        assert!(
            !law.is_poisoned(),
            "{mode}: a refused answer is a miss only"
        );
        r
    };
    assert_eq!(rule("ok"), None);
    for (mode, want) in [
        ("wrong_drive_count", SineMimoOutputRule::WrongDriveCount),
        ("nan", SineMimoOutputRule::NonFinite),
        ("infinite", SineMimoOutputRule::NonFinite),
        ("too_many_values", SineMimoOutputRule::TooManyValues),
        ("undeclared", SineMimoOutputRule::UndeclaredValue),
        ("bad_request", SineMimoOutputRule::BadRequest),
        (
            "wrong_reference_count",
            SineMimoOutputRule::WrongReferenceCount,
        ),
    ] {
        assert_eq!(rule(mode), Some(want), "{mode}");
    }

    // A drive over its ceiling is passed through for the core to scale.
    let mut law = misbehave(&host, "over_ceiling");
    law.engage(&inputs(&s, 0), BUDGET).unwrap();
    let o = law.tick(&inputs(&s, 1), BUDGET).unwrap();
    assert_eq!(o.drive[2].re, 6.0);

    // The law's own refusal, on a tick or of the run.
    let mut law = misbehave(&host, "law_error");
    law.engage(&inputs(&s, 0), BUDGET).unwrap();
    match law.tick(&inputs(&s, 1), BUDGET) {
        Err(LawError::Law(m)) => assert!(m.contains("refused on purpose")),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert!(!law.is_poisoned(), "a refusal leaves the law usable");
    let built = host
        .load_sine_mimo(&law_bytes(MISBEHAVE))
        .unwrap()
        .instantiate(
            &s,
            &[Param {
                key: "mode".into(),
                value: ParamValue::Choice("new_error".into()),
            }],
            BUDGET,
        );
    assert!(matches!(built, Err(LawError::Law(m)) if m.contains("refused the run")));

    // A trap, in engage or on a tick, poisons the instance.
    let mut law = misbehave(&host, "engage_trap");
    assert!(matches!(
        law.engage(&inputs(&s, 0), BUDGET),
        Err(LawError::Trap(_))
    ));
    assert!(law.is_poisoned());
    let mut law = misbehave(&host, "trap");
    law.engage(&inputs(&s, 0), BUDGET).unwrap();
    match law.tick(&inputs(&s, 1), BUDGET) {
        Err(LawError::Trap(m)) => assert!(m.contains("panicked on purpose"), "{m}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert!(matches!(
        law.tick(&inputs(&s, 2), BUDGET),
        Err(LawError::Poisoned(_))
    ));

    // Memory past the cap.
    let mut law = misbehave(&host, "grow");
    law.engage(&inputs(&s, 0), BUDGET).unwrap();
    assert!(matches!(
        law.tick(&inputs(&s, 1), BUDGET),
        Err(LawError::Memory { .. })
    ));
    assert!(matches!(
        law.tick(&inputs(&s, 2), BUDGET),
        Err(LawError::Poisoned(_))
    ));

    // Inputs that break the setup's shape never reach the law.
    let mut law = misbehave(&host, "ok");
    law.engage(&inputs(&s, 0), BUDGET).unwrap();
    let mut bad = inputs(&s, 1);
    bad.drive_in_force.pop();
    assert!(matches!(
        law.tick(&bad, BUDGET),
        Err(LawError::InputsInvalid(_))
    ));
    assert!(!law.is_poisoned());
}

#[test]
fn mimo_sine_a_late_law_is_stopped_at_its_budget_and_stays_stopped() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let s = setup(TestType::Sweep);
    let budget = Duration::from_millis(50);
    for mode in ["spin", "spin_after"] {
        let mut law = misbehave(&host, mode);
        law.engage(&inputs(&s, 0), budget).unwrap();
        if mode == "spin_after" {
            law.tick(&inputs(&s, 1), budget).unwrap();
        }
        let t0 = Instant::now();
        let r = law.tick(&inputs(&s, 2), budget);
        let took = t0.elapsed();
        assert!(
            matches!(r, Err(LawError::Deadline { budget: b }) if b == budget),
            "{mode}"
        );
        assert!(took >= budget, "{mode}: stopped early, after {took:?}");
        assert!(matches!(
            law.tick(&inputs(&s, 3), budget),
            Err(LawError::Poisoned(_))
        ));
    }
    // The constructor has its own budget.
    let law = host.load_sine_mimo(&law_bytes(MISBEHAVE)).unwrap();
    let r = law.instantiate(
        &s,
        &[Param {
            key: "mode".into(),
            value: ParamValue::Choice("new_spin".into()),
        }],
        budget,
    );
    assert!(matches!(r, Err(LawError::Deadline { budget: b }) if b == budget));
}

#[test]
fn mimo_sine_a_law_file_is_built_for_24_controls_12_drives_8_limits_and_no_more() {
    assert_eq!(
        (
            hertzforge_law_host::sine_mimo::MAX_SINE_MIMO_CONTROLS,
            hertzforge_law_host::sine_mimo::MAX_SINE_MIMO_DRIVES,
            hertzforge_law_host::sine_mimo::MAX_SINE_MIMO_LIMITS,
        ),
        (24, 12, 8)
    );
    let host = LawHost::new(HostConfig::default()).unwrap();
    // The template is a law file as a third party writes it from the SDK:
    // nothing in it names a size.
    let law = host.load_sine_mimo(&law_bytes(TEMPLATE)).unwrap();
    for (nc, nd) in [(24, 12), (12, 12), (24, 1), (1, 12)] {
        let s = sized(nc, nd, 8, TestType::Dwell);
        let mut i = law.instantiate(&s, &[], BUDGET).unwrap();
        let first = i.engage(&inputs(&s, 0), BUDGET).unwrap();
        assert_eq!(first.drive.len(), nd as usize, "{nc} by {nd}");
        let next = i.tick(&inputs(&s, 1), BUDGET).unwrap();
        assert_eq!(next.drive.len(), nd as usize, "{nc} by {nd}");
        // The template steps each pair's drive off the drive in force (0.1)
        // and leaves the rest: min(controls, drives) of them move, the
        // highest-numbered pair included.
        let moved: Vec<usize> = (0..nd as usize)
            .filter(|&j| next.drive[j].re != 0.1 || next.drive[j].im != 0.0)
            .collect();
        let pairs = nc.min(nd) as usize;
        assert_eq!(moved, (0..pairs).collect::<Vec<_>>(), "{nc} by {nd}");
    }
    for (nc, nd, nl, want) in [
        (25, 12, 0, "25 controls; 1 to 24 are accepted"),
        (24, 13, 0, "13 drives; 1 to 12 are accepted"),
        (24, 12, 9, "9 limit channels; 0 to 8 are accepted"),
    ] {
        match law.instantiate(&sized(nc, nd, nl, TestType::Dwell), &[], BUDGET) {
            Err(LawError::SetupInvalid(m)) => assert!(m.contains(want), "{m}"),
            Err(e) => panic!("{nc} by {nd} by {nl}: {e}"),
            Ok(_) => panic!("{nc} by {nd} by {nl} was built"),
        }
    }
}
