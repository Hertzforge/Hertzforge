//! Validation of the contracts: every rule refuses by name, and a good setup,
//! good inputs and good outputs pass.

use crate::engine::*;
use crate::law::*;
use crate::pulse::*;
use crate::setup::*;
use crate::{MAX_CONTROLS, MAX_DRIVES, MAX_LAW_VALUES, MAX_RECORD_LEN};
use hertzforge_sdm::{MatrixStack, C64};
use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault, LawValue};
use std::sync::Arc;

const LEN: usize = 64;
const NC: usize = 3;
const ND: usize = 2;

fn plant(nc: usize, nd: usize, len: usize) -> PlantStack {
    let lines = len / 2 + 1;
    let mut frf = MatrixStack::zeros(lines, nc, nd);
    for l in 0..lines {
        for c in 0..nc {
            frf.set(l, c, c % nd, C64::new(1.0, 0.0));
        }
    }
    PlantStack {
        frf,
        coherence: vec![0.99; nc * lines],
        trusted: vec![true; lines],
    }
}

fn waveform_setup() -> Setup {
    Setup {
        fs_hz: 8192.0,
        record_len: LEN,
        n_drives: ND,
        n_controls: NC,
        band_lo_hz: 10.0,
        band_hi_hz: 2000.0,
        target: Target::Waveform(WaveformTarget {
            records: vec![0.1; NC * LEN],
        }),
        timing: Timing::Free { window_s: 0.002 },
        plant: plant(NC, ND, LEN),
        drive_ceiling_v: vec![1.0; ND],
        ladder_db: vec![-12.0, -9.0, -6.0, -3.0, 0.0],
    }
}

fn srs_setup() -> Setup {
    let mut s = waveform_setup();
    let comp = SrsComponent {
        hz: 400.0,
        amplitude: 1.0,
        delay_s: 0.0,
        shape: ComponentShape::HalfCycles(5),
    };
    s.target = Target::Srs(SrsTarget {
        natural_hz: vec![100.0, 200.0, 400.0],
        q: 10.0,
        points: vec![5.0; NC * 3],
        tol_lo_db: -3.0,
        tol_hi_db: 3.0,
        waveforms: vec![0.2; NC * LEN],
        components: vec![vec![comp]; NC],
    });
    s
}

fn rule_of(s: &Setup) -> SetupRule {
    s.validate().expect_err("refused").rule
}

#[test]
fn mimo_shock_good_setups_pass() {
    waveform_setup().validate().unwrap();
    srs_setup().validate().unwrap();
    let mut s = waveform_setup();
    s.timing = Timing::Hold(HoldTiming {
        delays_s: vec![0.0, 0.001, 0.002],
        window_s: 0.001,
    });
    s.validate().unwrap();
    assert_eq!(s.lines(), 33);
    assert_eq!(s.df_hz(), 128.0);
}

#[test]
fn mimo_shock_setup_sizes_are_limited() {
    let mut s = waveform_setup();
    s.n_controls = 0;
    assert_eq!(rule_of(&s), SetupRule::ControlsOutOfRange);
    s.n_controls = MAX_CONTROLS + 1;
    assert_eq!(rule_of(&s), SetupRule::ControlsOutOfRange);
    let mut s = waveform_setup();
    s.n_drives = 0;
    assert_eq!(rule_of(&s), SetupRule::DrivesOutOfRange);
    s.n_drives = MAX_DRIVES + 1;
    assert_eq!(rule_of(&s), SetupRule::DrivesOutOfRange);
    for len in [0, 48, 32, 2 * MAX_RECORD_LEN] {
        let mut s = waveform_setup();
        s.record_len = len;
        assert_eq!(rule_of(&s), SetupRule::RecordLenInvalid, "{len}");
    }
}

#[test]
fn mimo_shock_setup_rate_band_and_target_are_checked() {
    let mut s = waveform_setup();
    s.fs_hz = f64::NAN;
    assert_eq!(rule_of(&s), SetupRule::RateInvalid);
    let mut s = waveform_setup();
    s.band_hi_hz = 5000.0;
    assert_eq!(rule_of(&s), SetupRule::BandInvalid);
    let mut s = waveform_setup();
    s.band_lo_hz = 3000.0;
    assert_eq!(rule_of(&s), SetupRule::BandInvalid);
    let mut s = waveform_setup();
    s.target = Target::Waveform(WaveformTarget {
        records: vec![0.0; NC * LEN - 1],
    });
    assert_eq!(rule_of(&s), SetupRule::TargetShape);
    let mut s = waveform_setup();
    s.target = Target::Waveform(WaveformTarget {
        records: vec![f64::INFINITY; NC * LEN],
    });
    assert_eq!(rule_of(&s), SetupRule::TargetNotFinite);
}

#[test]
fn mimo_shock_srs_target_rules_are_checked() {
    let edit = |f: &dyn Fn(&mut SrsTarget)| {
        let mut s = srs_setup();
        if let Target::Srs(t) = &mut s.target {
            f(t);
        }
        rule_of(&s)
    };
    assert_eq!(edit(&|t| t.natural_hz = vec![]), SetupRule::SrsFrequencies);
    assert_eq!(
        edit(&|t| t.natural_hz = vec![200.0, 100.0, 400.0]),
        SetupRule::SrsFrequencies
    );
    assert_eq!(edit(&|t| t.q = 0.5), SetupRule::SrsQ);
    assert_eq!(
        edit(&|t| t.points.pop().map(|_| ()).unwrap()),
        SetupRule::TargetShape
    );
    assert_eq!(edit(&|t| t.points[0] = 0.0), SetupRule::TargetNotFinite);
    assert_eq!(edit(&|t| t.tol_lo_db = 1.0), SetupRule::SrsTolerance);
    assert_eq!(
        edit(&|t| t.components.pop().map(|_| ()).unwrap()),
        SetupRule::TargetShape
    );
    assert_eq!(
        edit(&|t| t.components[1][0].shape = ComponentShape::HalfCycles(4)),
        SetupRule::SrsComponent
    );
    assert_eq!(
        edit(&|t| t.components[0][0].shape = ComponentShape::Decay(1.5)),
        SetupRule::SrsComponent
    );
    assert_eq!(
        edit(&|t| t.components[2][0].hz = 5000.0),
        SetupRule::SrsComponent
    );
    assert_eq!(
        edit(&|t| t.components[2][0].delay_s = -1.0),
        SetupRule::SrsComponent
    );
    assert_eq!(edit(&|t| t.waveforms.truncate(5)), SetupRule::TargetShape);
}

#[test]
fn mimo_shock_setup_timing_plant_ceilings_and_ladder_are_checked() {
    let mut s = waveform_setup();
    s.timing = Timing::Free { window_s: 0.0 };
    assert_eq!(rule_of(&s), SetupRule::Timing);
    s.timing = Timing::Hold(HoldTiming {
        delays_s: vec![0.0; NC - 1],
        window_s: 0.001,
    });
    assert_eq!(rule_of(&s), SetupRule::Timing);

    let mut s = waveform_setup();
    s.plant = plant(NC, ND + 1, LEN);
    assert_eq!(rule_of(&s), SetupRule::PlantShape);
    s.plant = plant(NC, ND, 2 * LEN);
    assert_eq!(rule_of(&s), SetupRule::PlantShape);
    let mut s = waveform_setup();
    s.plant.frf.set(3, 0, 1, C64::new(f64::NAN, 0.0));
    assert_eq!(rule_of(&s), SetupRule::PlantNotFinite);
    let mut s = waveform_setup();
    s.plant.coherence[4] = 1.5;
    assert_eq!(rule_of(&s), SetupRule::Coherence);
    let mut s = waveform_setup();
    s.plant.trusted.pop();
    assert_eq!(rule_of(&s), SetupRule::TrustedShape);

    let mut s = waveform_setup();
    s.drive_ceiling_v = vec![1.0];
    assert_eq!(rule_of(&s), SetupRule::DriveCeiling);
    s.drive_ceiling_v = vec![1.0, 0.0];
    assert_eq!(rule_of(&s), SetupRule::DriveCeiling);

    for ladder in [
        vec![],
        vec![-6.0, -6.0, 0.0],
        vec![-6.0, -3.0],
        vec![0.0, -3.0],
    ] {
        let mut s = waveform_setup();
        s.ladder_db = ladder.clone();
        assert_eq!(rule_of(&s), SetupRule::Ladder, "{ladder:?}");
    }
}

#[test]
fn mimo_shock_rule_names_are_stable_and_distinct() {
    use SetupRule as R;
    let all = [
        R::ControlsOutOfRange,
        R::DrivesOutOfRange,
        R::RecordLenInvalid,
        R::RateInvalid,
        R::BandInvalid,
        R::TargetShape,
        R::TargetNotFinite,
        R::SrsFrequencies,
        R::SrsQ,
        R::SrsTolerance,
        R::SrsComponent,
        R::Timing,
        R::PlantShape,
        R::PlantNotFinite,
        R::Coherence,
        R::TrustedShape,
        R::DriveCeiling,
        R::Ladder,
        R::TargetNotAccepted,
        R::DuplicateChannel,
    ];
    let mut names: Vec<&str> = all.iter().map(|r| r.as_str()).collect();
    for (r, n) in all.iter().zip(&names) {
        assert_eq!(serde_json::to_value(r).unwrap(), serde_json::json!(n));
    }
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), all.len());
    assert_eq!(TargetKind::Srs.as_str(), "srs");
    assert_eq!(TargetKind::Waveform.as_str(), "waveform");
}

fn inputs(s: &Setup) -> PulseInputs {
    let verdict = ControlVerdict {
        passed: true,
        error_db: 0.5,
        delay_s: 0.0,
        peak_ratio: 1.0,
    };
    PulseInputs {
        pulse_index: 1,
        level_db: -12.0,
        next_level_db: -9.0,
        drives: vec![0.0; s.n_drives * s.record_len],
        responses: vec![0.0; s.n_controls * s.record_len],
        srs: match &s.target {
            Target::Srs(t) => Some(vec![1.0; s.n_controls * t.natural_hz.len()]),
            Target::Waveform(_) => None,
        },
        verdicts: vec![verdict; s.n_controls],
        limited: false,
    }
}

#[test]
fn mimo_shock_inputs_are_checked_against_the_setup() {
    let s = waveform_setup();
    inputs(&s).validate(&s).unwrap();
    let srs = srs_setup();
    inputs(&srs).validate(&srs).unwrap();
    let rule = |s: &Setup, f: &dyn Fn(&mut PulseInputs)| {
        let mut i = inputs(s);
        f(&mut i);
        i.validate(s).expect_err("refused").rule
    };
    assert_eq!(rule(&s, &|i| i.pulse_index = 0), InputsRule::PulseIndex);
    assert_eq!(rule(&s, &|i| i.next_level_db = f64::NAN), InputsRule::Level);
    assert_eq!(rule(&s, &|i| i.drives.push(0.0)), InputsRule::DrivesLength);
    assert_eq!(
        rule(&s, &|i| i.responses.pop().map(|_| ()).unwrap()),
        InputsRule::ResponsesLength
    );
    assert_eq!(
        rule(&s, &|i| i.srs = Some(vec![1.0; 9])),
        InputsRule::SrsShape
    );
    assert_eq!(rule(&srs, &|i| i.srs = None), InputsRule::SrsShape);
    assert_eq!(
        rule(&srs, &|i| i.srs = Some(vec![1.0; 2])),
        InputsRule::SrsShape
    );
    assert_eq!(
        rule(&s, &|i| i.verdicts.pop().map(|_| ()).unwrap()),
        InputsRule::VerdictsLength
    );
    assert_eq!(
        rule(&s, &|i| i.responses[7] = f64::NAN),
        InputsRule::NonFinite
    );
}

#[test]
fn mimo_shock_the_joint_verdict_needs_every_control() {
    let s = waveform_setup();
    let mut i = inputs(&s);
    assert!(i.all_passed());
    i.verdicts[2].passed = false;
    assert!(!i.all_passed());
    i.verdicts.clear();
    assert!(!i.all_passed());
}

#[test]
fn mimo_shock_outputs_are_checked_against_the_setup_and_manifest() {
    let s = waveform_setup();
    let declared = vec!["gain".to_string()];
    let good = PulseOutputs {
        drives: vec![0.0; ND * LEN],
        delays_s: Some(vec![0.0; NC]),
        values: vec![LawValue::new("gain", 0.5)],
    };
    good.validate(&s, &declared).unwrap();
    let rule = |f: &dyn Fn(&mut PulseOutputs)| {
        let mut o = good.clone();
        f(&mut o);
        o.validate(&s, &declared).expect_err("refused").rule
    };
    assert_eq!(
        rule(&|o| o.drives.pop().map(|_| ()).unwrap()),
        OutputRule::WrongLength
    );
    assert_eq!(rule(&|o| o.drives[3] = f64::NAN), OutputRule::NonFinite);
    assert_eq!(
        rule(&|o| o.delays_s = Some(vec![0.0; NC + 1])),
        OutputRule::DelaysShape
    );
    assert_eq!(
        rule(&|o| o.values = vec![LawValue::new("other", 1.0)]),
        OutputRule::UndeclaredValue
    );
    assert_eq!(
        rule(&|o| o.values = vec![LawValue::new("gain", 1.0), LawValue::new("gain", 2.0)]),
        OutputRule::UndeclaredValue
    );
    assert_eq!(
        rule(&|o| o.values = vec![LawValue::new("gain", 1.0); MAX_LAW_VALUES + 1]),
        OutputRule::TooManyValues
    );
    assert_eq!(
        rule(&|o| o.values[0].value = f64::INFINITY),
        OutputRule::TooManyValues
    );
}

#[test]
fn mimo_shock_a_law_that_does_not_accept_the_target_kind_is_refused() {
    let m = LawManifest {
        targets: vec![TargetKind::Srs],
        values: Vec::new(),
    };
    check_law_accepts(&m, &srs_setup()).unwrap();
    let e = check_law_accepts(&m, &waveform_setup()).unwrap_err();
    assert_eq!(e.rule, SetupRule::TargetNotAccepted);
    assert!(e.message.contains("not waveform"), "{e}");
}

struct NoLaw;

impl MimoShockLawFactory for NoLaw {
    fn build(&self, _setup: &Setup) -> Result<Box<dyn MimoShockLaw + Send>, LawFault> {
        Err(LawFault::Failed("no law".into()))
    }
    fn describe(&self) -> LawDescriptor {
        LawDescriptor {
            id: "none".into(),
            name: "none".into(),
            version: "0.0.0".into(),
            sha256: None,
            params: serde_json::json!({}),
        }
    }
    fn manifest(&self) -> LawManifest {
        LawManifest {
            targets: TargetKind::ALL.to_vec(),
            values: Vec::new(),
        }
    }
}

fn config() -> MimoShockConfig {
    MimoShockConfig {
        fs_hz: 8192.0,
        record_len: LEN,
        band_lo_hz: 10.0,
        band_hi_hz: 2000.0,
        controls: vec![0, 1, 2],
        drives: vec![
            DriveChannel {
                channel: 4,
                ceiling_v: 0.5,
                abort_v: 0.8,
            },
            DriveChannel {
                channel: 5,
                ceiling_v: 0.5,
                abort_v: 0.8,
            },
        ],
        limits: vec![LimitChannel {
            channel: 3,
            abort_peak: 50.0,
        }],
        target: waveform_setup().target,
        timing: Timing::Free { window_s: 0.002 },
        ladder: LadderPlan::default(),
        identification: IdentificationPlan {
            drive_vrms: 0.05,
            frames: 20,
            coherence_floor: 0.5,
            review: true,
        },
        guards: GuardPlan::default(),
        min_spacing_s: 1.0,
    }
}

#[test]
fn mimo_shock_the_engine_refuses_a_bad_configuration_by_name() {
    let rule = |f: &dyn Fn(&mut MimoShockConfig)| {
        let mut c = config();
        f(&mut c);
        c.validate().expect_err("refused").rule
    };
    config().validate().unwrap();
    assert_eq!(rule(&|c| c.controls.clear()), SetupRule::ControlsOutOfRange);
    assert_eq!(rule(&|c| c.controls.push(4)), SetupRule::DuplicateChannel);
    assert_eq!(rule(&|c| c.drives.clear()), SetupRule::DrivesOutOfRange);
    assert_eq!(rule(&|c| c.record_len = 100), SetupRule::RecordLenInvalid);
    assert_eq!(
        rule(&|c| c.drives[1].abort_v = 0.5),
        SetupRule::DriveCeiling
    );
    assert_eq!(
        rule(&|c| c.ladder.levels_db = vec![-3.0, 3.0]),
        SetupRule::Ladder
    );
    assert_eq!(rule(&|c| c.record_len = 2 * LEN), SetupRule::TargetShape);
    let e = MimoShockEngine::new(
        MimoShockConfig {
            drives: Vec::new(),
            ..config()
        },
        Arc::new(NoLaw),
    )
    .err()
    .expect("refused");
    assert!(matches!(e, EngineError::Config(_)), "{e}");
}

#[test]
fn mimo_shock_engine_steps_out_of_order_fail_loudly_and_change_nothing() {
    let mut e = MimoShockEngine::new(config(), Arc::new(NoLaw)).unwrap();
    assert_eq!(e.state(), RunState::Idle);
    assert_eq!(e.law().id, "none");
    let idle = EngineError::WrongState(RunState::Idle);
    let calls: [Result<(), EngineError>; 4] = [e.identify(), e.proceed(), e.stop(), e.abort()];
    for r in calls {
        assert_eq!(r, Err(idle.clone()));
    }
    let fired = e.fire_next(None).unwrap_err();
    assert_eq!(fired, idle);
    assert!(fired.to_string().contains("not allowed while idle"));
    assert_eq!(e.state(), RunState::Idle);
    assert!(e.pulses().is_empty());
}

#[test]
fn mimo_shock_run_state_phases_follow_the_classical_vocabulary() {
    assert_eq!(RunState::Identifying.phase(), "sysid");
    assert_eq!(RunState::AwaitingReview.phase(), "awaiting_review");
    assert_eq!(
        serde_json::to_value(EndOutcome::NotConverged).unwrap(),
        serde_json::json!("not_converged")
    );
}
