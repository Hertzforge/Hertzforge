use crate::law::{LawDescriptor, LawValues};
use crate::*;
use hertzforge_sdm::{MatrixStack, C64};
use hertzforge_sine::profile::{ProfileRow, SineProfile};
use hertzforge_sine::scheduler::{
    BidirSweepConfig, DwellFor, DwellPlan, LadderConfig, RungFor, StepSineConfig, StepSpacing,
    SweepDirection, SweepTarget,
};
use hertzforge_sine::SweepRate;
use std::sync::Arc;

fn profile(g: f64) -> SineProfile {
    let mut p = SineProfile::new("flat");
    p.push(ProfileRow::accel(5.0, g));
    p.push(ProfileRow::accel(2000.0, g));
    p
}

fn sweep() -> TestPlan {
    TestPlan::Sweep(BidirSweepConfig {
        band: (10.0, 1000.0),
        rate: SweepRate::LogOctPerMin(2.0),
        start_hz: 10.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(2),
    })
}

fn setup(nc: usize, nd: usize) -> MimoSineSetup {
    MimoSineSetup {
        controls: (0..nc)
            .map(|i| ControlChannel {
                channel: 1 + i,
                profile: profile(1.0),
                phase: vec![PhasePoint {
                    freq_hz: 10.0,
                    phase_deg: 0.0,
                }],
            })
            .collect(),
        drives: (0..nd)
            .map(|i| DriveChannel {
                channel: 20 + i,
                ceiling_vpk: 2.0,
                abort_vpk: 3.0,
            })
            .collect(),
        limits: vec![LimitChannel {
            channel: 40,
            profile: profile(3.0),
        }],
        phase_specified: true,
        test: sweep(),
        schedule: Schedule {
            mode: LevelMode::Auto,
            ladder: LadderConfig {
                sysid_level_db: -20.0,
                start_db: -12.0,
                table_db: vec![-12.0, -6.0, 0.0],
                per_rung: RungFor::Sweeps(1),
                full_for: RungFor::Sweeps(1),
            },
        },
        fs_hz: 8192.0,
        tick_s: 0.01,
        tracking: Tracking::default(),
        identification: IdentificationSettings::default(),
        law: LawChoice::default(),
        requests: RequestSettings::default(),
        guards: GuardSettings::default(),
    }
}

fn rule(s: &MimoSineSetup) -> SetupRule {
    s.validate().expect_err("the setup should be refused").rule
}

#[test]
fn mimo_sine_a_well_formed_setup_passes_and_states_its_shape_and_type() {
    let s = setup(4, 4);
    s.validate().unwrap();
    assert_eq!(s.shape(), MimoShape::Square);
    assert_eq!(s.test_type(), TestType::Sweep);
    assert_eq!(setup(6, 3).shape(), MimoShape::MoreControlsThanDrives);
    assert_eq!(setup(3, 6).shape(), MimoShape::FewerControlsThanDrives);
    setup(16, 8).validate().unwrap();
}

#[test]
fn mimo_sine_the_defaults_are_the_design_defaults() {
    let g = GuardSettings::default();
    assert_eq!(
        g.rotation_cap_deg, None,
        "the rotation cap is off by default"
    );
    assert_eq!(g.max_step_db, 1.0);
    assert_eq!(g.max_rise_db_per_s, 240.0);
    assert_eq!(g.law_budget, 0.25);
    assert_eq!(g.law_misses, 3);
    assert_eq!(g.divergence_s, 1.0);
    assert_eq!(g.notch.near_bound_db, 3.0);
    let r = RequestSettings::default();
    assert!(!r.honour, "requests are recorded, not honoured, by default");
    assert_eq!(r.max_hold_s, 2.0);
    assert_eq!(r.min_slow_factor, 0.1);
    assert_eq!(LawChoice::default().id, "matrix");
    assert_eq!(BUILTIN_LAW_IDS[0], DEFAULT_LAW_ID);
}

#[test]
fn mimo_sine_sizes_are_capped_at_16_controls_8_drives_8_limits() {
    assert_eq!(rule(&setup(17, 4)), SetupRule::ControlsOutOfRange);
    assert_eq!(rule(&setup(0, 4)), SetupRule::ControlsOutOfRange);
    assert_eq!(rule(&setup(4, 9)), SetupRule::DrivesOutOfRange);
    assert_eq!(rule(&setup(4, 0)), SetupRule::DrivesOutOfRange);
    let mut s = setup(4, 4);
    s.limits = (0..9)
        .map(|i| LimitChannel {
            channel: 40 + i,
            profile: profile(3.0),
        })
        .collect();
    assert_eq!(rule(&s), SetupRule::LimitsOutOfRange);
    let mut s = setup(4, 4);
    s.limits[0].channel = 1;
    let e = s.validate().unwrap_err();
    assert_eq!(e.rule, SetupRule::DuplicateChannel);
    assert_eq!(e.field, "limits[0].channel");
}

#[test]
fn mimo_sine_drive_ceilings_and_aborts_are_checked() {
    let mut s = setup(2, 2);
    s.drives[1].ceiling_vpk = 0.0;
    assert_eq!(rule(&s), SetupRule::CeilingInvalid);
    let mut s = setup(2, 2);
    s.drives[0].abort_vpk = 2.0;
    let e = s.validate().unwrap_err();
    assert_eq!(e.rule, SetupRule::AbortBelowCeiling);
    assert_eq!(e.field, "drives[0].abort_vpk");
    let mut s = setup(2, 2);
    s.drives[0].abort_vpk = 11.0;
    assert_eq!(rule(&s), SetupRule::AbortBelowCeiling);
}

#[test]
fn mimo_sine_references_phases_and_bounds_are_checked() {
    let mut s = setup(2, 2);
    s.controls[1].profile = SineProfile::new("empty");
    assert_eq!(rule(&s), SetupRule::ProfileInvalid);
    let mut s = setup(2, 2);
    let mut p = SineProfile::new("short");
    p.push(ProfileRow::accel(20.0, 1.0));
    p.push(ProfileRow::accel(500.0, 1.0));
    s.controls[0].profile = p.clone();
    assert_eq!(rule(&s), SetupRule::ProfileNotCovering);
    let mut s = setup(2, 2);
    s.limits[0].profile = p;
    let e = s.validate().unwrap_err();
    assert_eq!(e.rule, SetupRule::ProfileNotCovering);
    assert_eq!(e.field, "limits[0].profile");
    let mut s = setup(2, 2);
    s.controls[0].phase.clear();
    assert_eq!(rule(&s), SetupRule::PhaseMissing);
    // An amplitude-only run needs no phase.
    s.phase_specified = false;
    s.validate().unwrap();
    let mut s = setup(2, 2);
    s.controls[0].phase.push(PhasePoint {
        freq_hz: 5.0,
        phase_deg: 10.0,
    });
    assert_eq!(rule(&s), SetupRule::PhaseInvalid);
    let mut s = setup(2, 2);
    s.controls[0].phase[0].phase_deg = -180.0;
    assert_eq!(rule(&s), SetupRule::PhaseInvalid);
}

#[test]
fn mimo_sine_each_test_type_is_checked_by_its_own_scheduler() {
    let mut s = setup(2, 2);
    s.test = TestPlan::Dwell(DwellPlan {
        freq_hz: 100.0,
        for_: DwellFor::Seconds(30.0),
    });
    s.validate().unwrap();
    assert_eq!(s.test.band_hz(), (100.0, 100.0));
    assert_eq!(s.test_type(), TestType::Dwell);
    s.test = TestPlan::Dwell(DwellPlan {
        freq_hz: 100.0,
        for_: DwellFor::Cycles(0),
    });
    assert_eq!(rule(&s), SetupRule::TestInvalid);
    s.test = TestPlan::Stepped(StepPlan::Grid(StepSineConfig {
        band: (20.0, 400.0),
        steps_per_octave: 4.0,
        spacing: StepSpacing::Log,
        dwell: Some(DwellFor::Seconds(2.0)),
    }));
    s.validate().unwrap();
    assert_eq!(s.test_type(), TestType::Stepped);
    s.test = TestPlan::Stepped(StepPlan::List {
        frequencies_hz: vec![50.0, 20.0, 300.0],
        dwell: DwellFor::Cycles(200),
    });
    s.validate().unwrap();
    assert_eq!(s.test.band_hz(), (20.0, 300.0));
    s.test = TestPlan::Stepped(StepPlan::List {
        frequencies_hz: vec![],
        dwell: DwellFor::Cycles(200),
    });
    assert_eq!(rule(&s), SetupRule::TestInvalid);
    s.test = TestPlan::Stepped(StepPlan::List {
        frequencies_hz: vec![50.0, -1.0],
        dwell: DwellFor::Cycles(200),
    });
    assert_eq!(s.validate().unwrap_err().field, "test.frequencies_hz[1]");
    let mut s = setup(2, 2);
    if let TestPlan::Sweep(c) = &mut s.test {
        c.band = (100.0, 50.0);
    }
    assert_eq!(rule(&s), SetupRule::TestInvalid);
    let mut s = setup(2, 2);
    s.fs_hz = 2048.0;
    assert_eq!(rule(&s), SetupRule::BandAboveGrid);
}

#[test]
fn mimo_sine_rate_tick_schedule_and_settings_are_checked() {
    let mut s = setup(2, 2);
    s.fs_hz = 500.0;
    assert_eq!(rule(&s), SetupRule::RateOutOfRange);
    let mut s = setup(2, 2);
    s.tick_s = 0.001;
    assert_eq!(rule(&s), SetupRule::TickOutOfRange);
    let mut s = setup(2, 2);
    s.schedule.ladder.table_db = vec![-6.0, -12.0, 0.0];
    assert_eq!(rule(&s), SetupRule::ScheduleInvalid);
    let mut s = setup(2, 2);
    s.schedule.ladder.start_db = -20.0;
    assert_eq!(rule(&s), SetupRule::ScheduleInvalid);
    let mut s = setup(2, 2);
    s.tracking = Tracking::Proportional {
        pct: 0.0,
        max_hz: 100.0,
    };
    assert_eq!(rule(&s), SetupRule::TrackingInvalid);
    let mut s = setup(2, 2);
    s.identification.level_db = 3.0;
    assert_eq!(rule(&s), SetupRule::IdentificationInvalid);
    let mut s = setup(2, 2);
    s.law.id = " ".into();
    assert_eq!(rule(&s), SetupRule::LawUnknown);
    let mut s = setup(2, 2);
    s.requests.max_hold_s = 2.5;
    assert_eq!(rule(&s), SetupRule::RequestBoundsInvalid);
    let mut s = setup(2, 2);
    s.requests.min_slow_factor = 0.05;
    assert_eq!(rule(&s), SetupRule::RequestBoundsInvalid);
    let mut s = setup(2, 2);
    s.guards.rotation_cap_deg = Some(0.0);
    let e = s.validate().unwrap_err();
    assert_eq!(e.rule, SetupRule::GuardInvalid);
    assert_eq!(e.field, "guards.rotation_cap_deg");
    s.guards.rotation_cap_deg = Some(15.0);
    s.validate().unwrap();
    let mut s = setup(2, 2);
    s.guards.max_step_db = 0.5;
    assert_eq!(rule(&s), SetupRule::GuardInvalid);
}

#[test]
fn mimo_sine_a_setup_survives_its_json() {
    let s = setup(3, 2);
    let back: MimoSineSetup = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back, s);
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["test"]["kind"], "sweep");
    assert_eq!(v["guards"]["rotation_cap_deg"], serde_json::Value::Null);
}

fn manifest() -> LawManifest {
    LawManifest {
        shapes: vec![MimoShape::Square],
        test_types: vec![TestType::Sweep, TestType::Dwell],
        amplitude_only: false,
        values: vec![],
    }
}

#[test]
fn mimo_sine_a_law_refuses_a_shape_a_test_type_or_a_reference_it_does_not_accept() {
    let m = manifest();
    check_law_accepts(&m, MimoShape::Square, TestType::Sweep, true).unwrap();
    let e = check_law_accepts(&m, MimoShape::MoreControlsThanDrives, TestType::Sweep, true);
    assert_eq!(e.unwrap_err().rule, SetupRule::LawShapeNotAccepted);
    let e = check_law_accepts(&m, MimoShape::Square, TestType::Stepped, true).unwrap_err();
    assert_eq!(e.rule, SetupRule::LawTestTypeNotAccepted);
    assert!(
        e.message.contains("sweep and dwell, not stepped"),
        "{}",
        e.message
    );
    let e = check_law_accepts(&m, MimoShape::Square, TestType::Dwell, false);
    assert_eq!(e.unwrap_err().rule, SetupRule::LawAmplitudeOnlyNotAccepted);
}

fn c(re: f64, im: f64) -> C64 {
    C64::new(re, im)
}

#[test]
fn mimo_sine_each_answer_rule_is_named() {
    let mut o = TickOutputs::new(3, 2);
    check_outputs(&o, 3, 2).unwrap();
    assert_eq!(check_outputs(&o, 4, 2), Err(OutputRule::WrongDriveCount));
    o.drive[1] = c(f64::NAN, 0.0);
    assert_eq!(check_outputs(&o, 3, 2), Err(OutputRule::NonFinite));
    o.reset();
    o.request = Some(Request::Slow(0.0));
    assert_eq!(check_outputs(&o, 3, 2), Err(OutputRule::BadRequest));
    o.request = Some(Request::Slow(1.5));
    assert_eq!(check_outputs(&o, 3, 2), Err(OutputRule::BadRequest));
    o.request = Some(Request::Slow(0.5));
    check_outputs(&o, 3, 2).unwrap();
    o.request = Some(Request::Hold);
    check_outputs(&o, 3, 2).unwrap();
    o.notched = true;
    o.notched_reference.pop();
    assert_eq!(
        check_outputs(&o, 3, 2),
        Err(OutputRule::WrongReferenceCount)
    );
    o.notched_reference.push(c(0.0, f64::INFINITY));
    assert_eq!(check_outputs(&o, 3, 2), Err(OutputRule::NonFinite));
    o.reset();
    assert!(!o.notched && o.request.is_none());
    assert_eq!(o.drive.len(), 3);
    for (r, name) in [
        (OutputRule::WrongDriveCount, "wrong_drive_count"),
        (OutputRule::NonFinite, "non_finite"),
        (OutputRule::TooManyValues, "too_many_values"),
        (OutputRule::UndeclaredValue, "undeclared_value"),
        (OutputRule::BadRequest, "bad_request"),
        (OutputRule::WrongReferenceCount, "wrong_reference_count"),
    ] {
        assert_eq!(r.as_str(), name);
        assert_eq!(LawFault::BadOutput(r).reason(), name);
    }
}

fn law_setup(nc: usize, nd: usize, nl: usize) -> LawSetup {
    let lines = 4;
    LawSetup {
        controls: nc,
        drives: nd,
        limits: nl,
        fs_hz: 8192.0,
        tick_s: 0.01,
        band_lo_hz: 10.0,
        band_hi_hz: 1000.0,
        test_type: TestType::Sweep,
        shape: MimoShape::of(nc, nd),
        drive_ceiling_v: vec![2.0; nd],
        identification: LawIdentification {
            frequency_hz: vec![10.0, 100.0, 500.0, 1000.0],
            response: MatrixStack::zeros(lines, nc, nd),
            limit_response: MatrixStack::zeros(lines, nl, nd),
            coherence: vec![0.99; nc * lines],
            limit_coherence: vec![0.99; nl * lines],
            trusted: vec![true; lines],
        },
        identification_level_db: Some(-20.0),
        reference: vec![c(1.0, 0.0); nc * lines],
        phase_specified: true,
        limit_bounds: vec![Some(3.0); nl * lines],
        tolerance: LawTolerance {
            alarm_low_db: vec![1.5; nc * lines],
            alarm_high_db: vec![1.5; nc * lines],
            abort_low_db: vec![3.0; nc * lines],
            abort_high_db: vec![3.0; nc * lines],
        },
        tracking: Tracking::default(),
    }
}

fn law_rule(s: &LawSetup) -> LawSetupRule {
    s.validate()
        .expect_err("the law setup should be refused")
        .rule
}

#[test]
fn mimo_sine_a_law_setup_is_checked_against_its_counts() {
    law_setup(4, 4, 1).validate().unwrap();
    law_setup(3, 6, 0).validate().unwrap();
    assert_eq!(law_rule(&law_setup(17, 4, 0)), LawSetupRule::SizeOutOfRange);
    let mut s = law_setup(4, 4, 1);
    s.shape = MimoShape::MoreControlsThanDrives;
    assert_eq!(law_rule(&s), LawSetupRule::ShapeMismatch);
    let mut s = law_setup(4, 4, 1);
    s.identification.response = MatrixStack::zeros(4, 4, 3);
    assert_eq!(law_rule(&s), LawSetupRule::WrongLength);
    let mut s = law_setup(4, 4, 1);
    s.reference.pop();
    assert_eq!(law_rule(&s), LawSetupRule::WrongLength);
    let mut s = law_setup(4, 4, 1);
    s.limit_bounds[2] = None;
    s.validate().unwrap();
    s.limit_bounds[2] = Some(-1.0);
    assert_eq!(law_rule(&s), LawSetupRule::ValueInvalid);
    let mut s = law_setup(4, 4, 1);
    s.identification.frequency_hz[2] = 50.0;
    assert_eq!(law_rule(&s), LawSetupRule::GridNotAscending);
    let mut s = law_setup(4, 4, 1);
    s.band_hi_hz = 5000.0;
    assert_eq!(law_rule(&s), LawSetupRule::GridInvalid);
    let mut s = law_setup(4, 4, 1);
    s.tolerance.abort_high_db[0] = -1.0;
    assert_eq!(law_rule(&s), LawSetupRule::ValueInvalid);
}

/// A law that plays nothing: the trait is implementable as declared.
struct Silent;

impl MimoSineLaw for Silent {
    fn name(&self) -> &str {
        "silent"
    }
    fn engage(
        &mut self,
        _: &TickInputs<'_>,
        out: &mut TickOutputs,
        _: &mut LawValues,
    ) -> Result<(), LawFault> {
        out.drive.fill(C64::new(0.0, 0.0));
        Ok(())
    }
    fn tick(
        &mut self,
        i: &TickInputs<'_>,
        out: &mut TickOutputs,
        _: &mut LawValues,
    ) -> Result<(), LawFault> {
        out.drive.copy_from_slice(i.drive_in_force);
        out.request = Some(Request::Slow(0.5));
        Ok(())
    }
}

struct SilentFactory(LawManifest);

impl MimoSineLawFactory for SilentFactory {
    fn build(&self, _: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault> {
        Ok(Box::new(Silent))
    }
    fn describe(&self) -> LawDescriptor {
        LawDescriptor {
            id: "silent".into(),
            name: "silent".into(),
            version: "0.1.0".into(),
            sha256: None,
            params: vec![],
            diagnostics: vec![],
            live: vec![],
        }
    }
    fn manifest(&self) -> LawManifest {
        self.0.clone()
    }
}

#[test]
fn mimo_sine_a_native_law_runs_through_the_traits_and_the_engine_is_not_built() {
    let f = SilentFactory(manifest());
    let mut law = f.build(&law_setup(2, 2, 0)).unwrap();
    let zero = [c(0.0, 0.0); 2];
    let played = [c(0.1, 0.2), c(-0.3, 0.0)];
    let mut inputs = TickInputs {
        index: 0,
        dt_s: 0.01,
        frequency_hz: 20.0,
        sweep_hz_per_s: 0.5,
        sweep_oct_per_min: 2.0,
        level: 0.25,
        reference: &zero,
        controls: &zero,
        limits: &[],
        limit_bounds: &[],
        drive_in_force: &zero,
        hold: Hold::Free,
        notch_scale: 1.0,
        settling: true,
    };
    let mut out = TickOutputs::new(2, 2);
    let mut values = LawValues::new(0);
    law.engage(&inputs, &mut out, &mut values).unwrap();
    check_outputs(&out, 2, 2).unwrap();
    inputs.index = 1;
    inputs.drive_in_force = &played;
    out.reset();
    law.tick(&inputs, &mut out, &mut values).unwrap();
    assert_eq!(out.drive, played);
    assert_eq!(out.request, Some(Request::Slow(0.5)));

    let mut e = MimoSineEngine::new(setup(2, 2), Arc::new(SilentFactory(manifest()))).unwrap();
    assert_eq!(e.identify(), Err(EngineError::NotBuilt));
    assert_eq!(e.run().unwrap_err().reason(), "not_built");
    let refused = MimoSineEngine::new(setup(3, 2), Arc::new(SilentFactory(manifest())));
    match refused {
        Err(EngineError::Setup(e)) => assert_eq!(e.rule, SetupRule::LawShapeNotAccepted),
        _ => panic!("a law that does not accept the shape was not refused"),
    }
}
