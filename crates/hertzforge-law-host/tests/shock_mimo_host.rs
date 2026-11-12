//! The law host's `shock-mimo` world: a MIMO Shock law file loads, is told
//! apart from the other worlds, runs contained, and every way it can misbehave
//! is reported by name.

use hertzforge_law_host::shock_mimo::{
    Complex, ControlVerdict, MatrixStack, MimoShockInputs, MimoShockSetup, Plant, SrsTarget,
    Target, TargetKind, Timing, WaveformTarget,
};
use hertzforge_law_host::{
    drive_over_ceiling, AnyLaw, HostConfig, LawError, LawHost, Param, ParamValue,
    ShockMimoLawInstance, ShockMimoOutputRule, SHOCK_MIMO_WORLD,
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

const BUDGET: Duration = Duration::from_secs(5);
const LEN: u32 = 64;

fn setup(kind: TargetKind) -> MimoShockSetup {
    let (nc, nd) = (2u32, 3u32);
    let lines = LEN / 2 + 1;
    let wave: Vec<f64> = (0..nc * LEN)
        .map(|i| (f64::from(i % LEN) * 0.3).sin())
        .collect();
    MimoShockSetup {
        fs_hz: 1024.0,
        record_len: LEN,
        drives: nd,
        controls: nc,
        band_lo_hz: 10.0,
        band_hi_hz: 400.0,
        target: match kind {
            TargetKind::Waveform => Target::Waveform(WaveformTarget { records: wave }),
            TargetKind::Srs => Target::Srs(SrsTarget {
                natural_hz: vec![100.0, 200.0],
                q: 10.0,
                points: vec![1.0; 4],
                tol_lo_db: -1.5,
                tol_hi_db: 3.0,
                waveforms: wave,
                components: vec![Vec::new(), Vec::new()],
            }),
        },
        timing: Timing::Free(0.002),
        plant: Plant {
            frf: MatrixStack {
                lines,
                rows: nc,
                cols: nd,
                data: (0..lines * nc * nd)
                    .map(|i| Complex {
                        re: 1.0 + f64::from(i % 3),
                        im: 0.0,
                    })
                    .collect(),
            },
            coherence: vec![1.0; (nc * lines) as usize],
            trusted: vec![true; lines as usize],
        },
        drive_ceiling_v: vec![1.0, 2.0, 3.0],
        ladder_db: vec![-6.0, 0.0],
        identification_level_db: f64::NAN,
    }
}

fn inputs(s: &MimoShockSetup, pulse_index: u64) -> MimoShockInputs {
    let srs = match &s.target {
        Target::Srs(t) => Some(vec![0.5; t.points.len()]),
        Target::Waveform(_) => None,
    };
    MimoShockInputs {
        pulse_index,
        level_db: -6.0,
        next_level_db: 0.0,
        drives: vec![0.01; (s.drives * LEN) as usize],
        responses: vec![0.02; (s.controls * LEN) as usize],
        srs,
        verdicts: vec![
            ControlVerdict {
                passed: false,
                error_db: 3.0,
                delay_s: 0.0,
                peak_ratio: 0.8,
            };
            s.controls as usize
        ],
        limited: false,
    }
}

fn misbehave(host: &LawHost, mode: &str) -> ShockMimoLawInstance {
    let law = host
        .load_shock_mimo(&law_bytes("law_fixture_shock_mimo_misbehave"))
        .unwrap();
    law.instantiate(
        &setup(TargetKind::Waveform),
        &[Param {
            key: "mode".into(),
            value: ParamValue::Choice(mode.into()),
        }],
        BUDGET,
    )
    .unwrap()
}

#[test]
fn mimo_shock_the_world_is_named_shock_mimo() {
    assert_eq!(SHOCK_MIMO_WORLD, "shock-mimo");
}

#[test]
fn mimo_shock_law_files_load_as_their_world_and_state_their_targets() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let both = vec![TargetKind::Waveform, TargetKind::Srs];
    for (name, targets) in [
        ("hertzforge_law_mimo_shock_paired", both.clone()),
        ("hertzforge_law_mimo_shock_inverse", both.clone()),
        ("hertzforge_law_mimo_shock_shape", vec![TargetKind::Srs]),
        ("law_fixture_shock_mimo_minimal", both.clone()),
    ] {
        match host.load_any(&law_bytes(name)) {
            Ok(AnyLaw::ShockMimo(l)) => {
                assert_eq!(l.manifest().targets, targets, "{name}");
                assert_eq!(l.identity().sha256.len(), 64);
            }
            Ok(other) => panic!("{name} loaded as {}", other.world()),
            Err(e) => panic!("{name}: {e}"),
        }
        let any = host.load_any(&law_bytes(name)).unwrap();
        assert_eq!(any.world(), "shock-mimo");
    }
}

#[test]
fn mimo_shock_a_law_of_another_world_is_not_taken_for_one() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let srs = host.load_any(&law_bytes("hertzforge_law_shock_srs_ratio"));
    assert!(matches!(srs, Ok(AnyLaw::ShockSrs(_))));
    match host.load_shock_mimo(&law_bytes("hertzforge_law_shock_srs_ratio")) {
        Err(LawError::NotALaw(m)) => assert!(m.contains("single-output shock law"), "{m}"),
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("an SRS law loaded as a MIMO Shock law"),
    }
    match host.load_shock_mimo(&law_bytes("hertzforge_law_mimo_pinv")) {
        Err(LawError::NotALaw(m)) => assert!(m.contains("multiple-output Random"), "{m}"),
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("a MIMO Random law loaded as a MIMO Shock law"),
    }
}

#[test]
fn mimo_shock_a_law_runs_contained_and_answers_the_run_shape() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = host
        .load_shock_mimo(&law_bytes("hertzforge_law_mimo_shock_paired"))
        .unwrap();
    let s = setup(TargetKind::Srs);
    let mut i = law.instantiate(&s, &[], BUDGET).unwrap();
    // Every declared parameter is completed with its default.
    assert_eq!(i.params().len(), 3);
    let seed = i.seed(-6.0, BUDGET).unwrap();
    assert_eq!(seed.drives.len(), 3 * LEN as usize);
    assert_eq!(seed.values[0].key, "drive_rms_full_level");
    let next = i.iterate(&inputs(&s, 1), BUDGET).unwrap();
    assert_eq!(next.drives.len(), 3 * LEN as usize);
    assert!(next.drives.iter().all(|v| v.is_finite()));
    // The third drive is paired with no control (two controls): silent.
    assert!(next.drives[2 * LEN as usize..].iter().all(|v| *v == 0.0));
}

#[test]
fn mimo_shock_a_target_kind_the_law_does_not_accept_is_refused() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let shape = host
        .load_shock_mimo(&law_bytes("hertzforge_law_mimo_shock_shape"))
        .unwrap();
    match shape.instantiate(&setup(TargetKind::Waveform), &[], BUDGET) {
        Err(LawError::SetupInvalid(m)) => assert!(m.contains("accepts srs targets"), "{m}"),
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("shape was built for a waveform target"),
    }
    // A bad setup never reaches the law.
    let mut s = setup(TargetKind::Srs);
    s.drive_ceiling_v.pop();
    assert!(matches!(
        shape.instantiate(&s, &[], BUDGET),
        Err(LawError::SetupInvalid(_))
    ));
}

#[test]
fn mimo_shock_each_misbehaviour_is_reported_by_name() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let s = setup(TargetKind::Waveform);
    let rule = |mode: &str| -> Option<ShockMimoOutputRule> {
        let mut law = misbehave(&host, mode);
        law.seed(-6.0, BUDGET).unwrap();
        match law.iterate(&inputs(&s, 1), BUDGET) {
            Err(LawError::BadShockMimoOutput { rule, detail }) => {
                assert!(!detail.is_empty());
                Some(rule)
            }
            Ok(_) => None,
            Err(e) => panic!("{mode}: {e}"),
        }
    };
    assert_eq!(rule("ok"), None);
    assert_eq!(
        rule("wrong_drive_count"),
        Some(ShockMimoOutputRule::WrongDriveCount)
    );
    assert_eq!(rule("wrong_length"), Some(ShockMimoOutputRule::WrongLength));
    assert_eq!(rule("nan"), Some(ShockMimoOutputRule::NonFinite));
    assert_eq!(rule("infinite"), Some(ShockMimoOutputRule::NonFinite));
    assert_eq!(rule("bad_delays"), Some(ShockMimoOutputRule::DelaysShape));
    assert_eq!(
        rule("undeclared"),
        Some(ShockMimoOutputRule::UndeclaredValue)
    );
    for (rule, name) in [
        (ShockMimoOutputRule::WrongDriveCount, "wrong_drive_count"),
        (ShockMimoOutputRule::WrongLength, "wrong_length"),
        (ShockMimoOutputRule::NonFinite, "non_finite"),
        (ShockMimoOutputRule::DelaysShape, "delays_shape"),
        (ShockMimoOutputRule::TooManyValues, "too_many_values"),
        (ShockMimoOutputRule::UndeclaredValue, "undeclared_value"),
    ] {
        assert_eq!(rule.as_str(), name);
    }

    // The law's own refusal.
    let mut law = misbehave(&host, "law_error");
    law.seed(-6.0, BUDGET).unwrap();
    match law.iterate(&inputs(&s, 1), BUDGET) {
        Err(LawError::Law(m)) => assert!(m.contains("refused on purpose")),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert!(!law.is_poisoned(), "a refusal leaves the law usable");

    // A trap, on the seed or on an iteration, poisons the instance.
    let mut law = misbehave(&host, "seed_trap");
    assert!(matches!(law.seed(-6.0, BUDGET), Err(LawError::Trap(_))));
    assert!(law.is_poisoned());
    let mut law = misbehave(&host, "trap");
    law.seed(-6.0, BUDGET).unwrap();
    match law.iterate(&inputs(&s, 1), BUDGET) {
        Err(LawError::Trap(m)) => assert!(m.contains("panicked on purpose"), "{m}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert!(matches!(
        law.iterate(&inputs(&s, 2), BUDGET),
        Err(LawError::Poisoned(_))
    ));

    // Memory past the cap.
    let mut law = misbehave(&host, "grow");
    law.seed(-6.0, BUDGET).unwrap();
    assert!(matches!(
        law.iterate(&inputs(&s, 1), BUDGET),
        Err(LawError::Memory { .. })
    ));

    // A drive over its ceiling is passed through and named for the core.
    let mut law = misbehave(&host, "over_ceiling");
    law.seed(-6.0, BUDGET).unwrap();
    let o = law.iterate(&inputs(&s, 1), BUDGET).unwrap();
    let e = drive_over_ceiling(&o, &s).unwrap();
    assert_eq!((e.drive, e.peak_v, e.ceiling_v), (2, 6.0, 3.0));
    assert!(e.to_string().contains("over_ceiling"));

    // Inputs that break the setup's shape never reach the law.
    let mut law = misbehave(&host, "ok");
    law.seed(-6.0, BUDGET).unwrap();
    let mut bad = inputs(&s, 1);
    bad.responses.pop();
    assert!(matches!(
        law.iterate(&bad, BUDGET),
        Err(LawError::InputsInvalid(_))
    ));
}

#[test]
fn mimo_shock_a_late_law_is_stopped_at_its_budget_and_stays_stopped() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let s = setup(TargetKind::Waveform);
    let budget = Duration::from_millis(50);
    for mode in ["spin", "spin_after"] {
        let mut law = misbehave(&host, mode);
        law.seed(-6.0, budget).unwrap();
        if mode == "spin_after" {
            law.iterate(&inputs(&s, 1), budget).unwrap();
        }
        let t0 = Instant::now();
        let r = law.iterate(&inputs(&s, 2), budget);
        let took = t0.elapsed();
        assert!(
            matches!(r, Err(LawError::Deadline { budget: b }) if b == budget),
            "{mode}"
        );
        assert!(took >= budget, "{mode}: stopped early, after {took:?}");
        assert!(matches!(
            law.iterate(&inputs(&s, 3), budget),
            Err(LawError::Poisoned(_))
        ));
    }
}
