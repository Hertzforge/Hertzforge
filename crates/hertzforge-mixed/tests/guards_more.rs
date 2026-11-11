//! The rest of the guards, each shown to trip, on the lagged resonant plant of
//! `common`: the kinematic monitor (and its switch), input overload, the random
//! part's rms ceiling and abort, the floor's rms and line aborts, a band's abort
//! naming the band, limit channels lowering the random part only and their own
//! abort catching a tone, a floor law fault ending in the controlled stop, and
//! the engine's health counters.

mod common;
use common::*;
use hertzforge_mixed::{
    MixedConfig, MixedLawFactories, MixedOutcome, MixedPhase, MixedService, MixedState,
    MixedTripKind,
};
use hertzforge_random::{
    ControlError, ControlLaw, LawFault, LawSetup, LevelCommand, LimitKind, LimitProfile,
    LoopDiagnostics, LoopInputs, LoopOutputs,
};
use std::sync::Arc;

fn quick(mut c: MixedConfig) -> MixedConfig {
    c.soft_start_db = 0.0;
    c.floor_settle_s = 3.0;
    c
}

fn build(c: MixedConfig, table: hertzforge_channel_table::ChannelTable) -> MixedService {
    let mut s = MixedService::new(
        c,
        table,
        station(DELAY),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        1,
        None,
    )
    .unwrap()
    .with_block_frames(BLOCK);
    s.arm().unwrap();
    s.run_sysid(0).unwrap();
    assert!(s.coherence_gate().unwrap());
    s.proceed_after_review().unwrap();
    s
}

fn kind(s: &MixedService) -> Option<MixedTripKind> {
    s.trip().map(|t| t.kind.clone())
}

/// A 3 g tone at 60 Hz moves the control point at about 3 in/s; a 2.8 in/s
/// velocity abort on the channel ends the run on the velocity axis. (With the
/// core's own monitor switched off, the engine's raw-sample watchdog still
/// trips on the same axis: the raw-sample protection cannot be switched off.)
#[test]
fn a_velocity_limit_on_the_raw_samples_ends_the_run() {
    let mut t = table();
    t.channels[0].abort.velocity_ips = Some(2.8);
    let c = quick(config(vec![floor(0.01), tone("t1", fixed(60.0), 3.0)]));
    for on in [true, false] {
        let mut c = c.clone();
        c.limits.kinematic_monitor = on;
        let mut s = build(c, t.clone());
        run_for(&mut s, 15.0);
        assert!(
            matches!(kind(&s), Some(MixedTripKind::Kinematic { ref axis, .. }) if axis == "velocity"),
            "{:?}",
            s.trip()
        );
        assert_eq!(s.state(), MixedState::Aborted);
    }
}

#[test]
fn an_input_overload_ends_the_run_naming_the_channel() {
    let mut t = table();
    t.channels[0].range_max = 5.0;
    let c = quick(config(vec![floor(0.01), tone("t1", fixed(250.0), 6.0)]));
    let mut s = build(c, t);
    run_for(&mut s, 15.0);
    assert_eq!(kind(&s), Some(MixedTripKind::Overload { channel_index: 0 }));
}

#[test]
fn the_random_part_is_held_to_its_rms_ceiling_and_ends_the_run_at_its_abort() {
    // The floor wants about 1.1 V rms at the plant's gain here.
    let mut c = quick(config(vec![floor(0.01)]));
    c.limits.drive_ceiling_vrms = Some(0.5);
    c.limits.max_lines_out = usize::MAX;
    c.floor().unwrap();
    let mut s = build(c, table());
    run_for(&mut s, 6.0);
    let v = s.live_status().totals.drive_vrms;
    eprintln!("random part held to {v:.3} V rms under a 0.5 V ceiling");
    assert!(v <= 0.5 * 1.05 && v > 0.4, "{v}");
    let mut c = quick(config(vec![floor(0.01)]));
    c.limits.drive_abort_vrms = Some(0.5);
    let mut s = build(c, table());
    run_for(&mut s, 6.0);
    assert!(
        matches!(kind(&s), Some(MixedTripKind::RandomVrms { .. })),
        "{:?}",
        s.trip()
    );
}

#[test]
fn the_floor_rms_and_line_aborts_trip() {
    let mut c = quick(config(vec![floor(0.01)]));
    if let hertzforge_mixed::PartBody::Floor(f) = &mut c.parts[0].body {
        f.abort_db = (0.02, 0.02);
    }
    c.limits.max_lines_out = usize::MAX;
    let mut s = build(c, table());
    run_for(&mut s, 20.0);
    assert!(
        matches!(kind(&s), Some(MixedTripKind::FloorRmsAbort { .. })),
        "{:?}",
        s.trip()
    );
    let mut c = quick(config(vec![floor(0.01)]));
    if let hertzforge_mixed::PartBody::Floor(f) = &mut c.parts[0].body {
        f.abort_db = (1.0, 1.0);
    }
    c.limits.max_lines_out = 0;
    let mut s = build(c, table());
    run_for(&mut s, 20.0);
    assert!(
        matches!(kind(&s), Some(MixedTripKind::FloorAbort { .. })),
        "{:?}",
        s.trip()
    );
}

/// A limit channel (the control point itself) with a ceiling under a band's
/// lines: the notch lowers the band, never the floor beside it; the band then
/// reads low and its abort names it.
#[test]
fn a_limit_channel_lowers_the_random_part_and_a_starved_band_aborts_by_name() {
    let mut c = quick(config(vec![
        floor(0.01),
        band("b1", fixed(600.0), 0.2, 40.0),
    ]));
    let lines = 513;
    let mut ceiling = vec![f64::INFINITY; lines];
    for (k, c) in ceiling.iter_mut().enumerate() {
        let f = k as f64 * 4.0;
        if (585.0..=615.0).contains(&f) {
            *c = 0.02;
        }
    }
    c.limits.limit_profiles = vec![LimitProfile {
        channel_index: 0,
        kind: LimitKind::Response,
        resp_ceiling: ceiling,
    }];
    let mut s = build(c, table());
    let mut limited = false;
    for _ in 0..80 {
        if !run_for(&mut s, 0.5) {
            break;
        }
        let st = s.live_status();
        limited |= st.parts[0].limiting;
        assert!(st.floor.error_db_rms.is_none_or(|e| e.abs() < 1.0));
    }
    assert!(limited);
    let t = s.trip().expect("tripped");
    assert!(matches!(t.kind, MixedTripKind::BandAbort { .. }), "{t:?}");
    assert_eq!(t.part.as_deref(), Some("b1"));
}

/// A tone is not notched (design Q197): a limit channel whose ceiling sits
/// under a tone's line catches it with its own abort.
#[test]
fn a_limit_channel_abort_catches_a_tone_it_does_not_notch() {
    let mut c = quick(config(vec![floor(0.01), tone("t1", fixed(250.0), 2.0)]));
    let mut ceiling = vec![f64::INFINITY; 513];
    for (k, c) in ceiling.iter_mut().enumerate() {
        if (240.0..=260.0).contains(&(k as f64 * 4.0)) {
            *c = 0.002;
        }
    }
    c.limits.limit_profiles = vec![LimitProfile {
        channel_index: 0,
        kind: LimitKind::Response,
        resp_ceiling: ceiling,
    }];
    c.limits.max_lines_out = 3;
    let mut s = build(c, table());
    run_for(&mut s, 25.0);
    assert!(
        matches!(
            kind(&s),
            Some(MixedTripKind::LimitChannel {
                channel_index: 0,
                ..
            })
        ),
        "{:?}",
        s.trip()
    );
}

/// A floor law that seeds well and then fails every loop.
struct Failing(Vec<f64>);
impl ControlLaw for Failing {
    fn init(&mut self, i: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        let d = i
            .target_psd
            .iter()
            .zip(i.h_mag2)
            .map(|(t, h)| if *h > 0.0 { t / h } else { 0.0 })
            .collect::<Vec<_>>();
        self.0 = d.clone();
        Ok(LoopOutputs {
            drive_psd: d,
            level_cmd_db: 0.0,
            diagnostics: LoopDiagnostics::default(),
        })
    }
    fn iterate(&mut self, _: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        Err(LawFault::Failed("no".into()))
    }
    fn retarget_level(&mut self, _: LevelCommand) -> Result<LoopOutputs, LawFault> {
        Err(LawFault::Failed("no".into()))
    }
    fn name(&self) -> &str {
        "failing"
    }
}

#[test]
fn a_floor_law_fault_ends_the_run_by_the_controlled_stop() {
    let mut laws = MixedLawFactories::builtin();
    laws.floor = Arc::new(
        |_: &LawSetup<'_>| -> Result<Box<dyn ControlLaw + Send>, ControlError> {
            Ok(Box::new(Failing(Vec::new())))
        },
    );
    let c = quick(config(vec![floor(0.01)]));
    let mut s = engaged_with(c, DELAY, laws);
    assert!(s.engine_health().is_some());
    let mut ramp = false;
    for _ in 0..60 {
        let alive = run_for(&mut s, 0.25);
        ramp |= s.live_status().phase == MixedPhase::RampingDown;
        if !alive {
            break;
        }
    }
    assert!(ramp);
    assert!(
        matches!(kind(&s), Some(MixedTripKind::LawFault { .. })),
        "{:?}",
        s.trip()
    );
    assert_eq!(s.report_data().outcome, Some(MixedOutcome::LawFault));
    assert!(s.end_reason().unwrap().starts_with("Floor law fault"));
}

/// Between the identification and the engage the station is parked: no
/// engine, nothing played; proceeding restarts it and the run closes its loop.
#[test]
fn the_station_is_parked_through_the_gate_and_review_and_restarts_at_engage() {
    let mut s = MixedService::new(
        quick(config(vec![floor(0.01), tone("t1", fixed(250.0), 2.0)])),
        table(),
        station(DELAY),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        1,
        None,
    )
    .unwrap()
    .with_block_frames(BLOCK);
    s.arm().unwrap();
    assert!(s.engine_health().is_some());
    s.run_sysid(0).unwrap();
    assert!(s.engine_health().is_none() && s.drive_is_zero());
    assert!(s.coherence_gate().unwrap());
    assert_eq!(s.state(), MixedState::AwaitingReview);
    assert!(s.engine_health().is_none());
    s.proceed_after_review().unwrap();
    assert!(s.engine_health().is_some());
    assert!(run_for(&mut s, 12.0));
    assert_eq!(s.live_status().phase, MixedPhase::Running);
}
