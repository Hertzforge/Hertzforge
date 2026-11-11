//! A tone's alarm and abort judge its reading averaged long enough for its
//! scatter. On the control service's demo plant (modes at 200 and 550 Hz, Q
//! 2.5, feedthrough 0.35) at 4096 Hz with 0.5 Hz lines, a 0.1 g tone on a
//! 5e-4 g^2/Hz floor stands about 13 dB over the floor in its tracker's band
//! and a single reading scatters about 1.3 dB: judged on single readings it
//! false-tripped its 3 dB abort within a minute; judged as now, it runs ten
//! minutes, dwelling or sweeping, with no alarm and no trip. A real 4 dB
//! shortfall on that tone still ends in its abort, and a large tone's abort
//! comes as fast as before.

mod common;
use common::table;
use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};
use hertzforge_hal_sim::SimConfig;
use hertzforge_mixed::{
    parts::{
        Direction, FloorPart, MixedIdentification, MixedLawChoice, MixedLimits, Part, PartBody,
        PartSchedule, PartSweepTarget, PartWindow, SweepLaw, ToneAmplitude, TonePart,
    },
    predict, MixedConfig, MixedLawFactories, MixedPhase, MixedService, MixedTripKind, PartKind,
};
use hertzforge_random::service::config::{ControlSet, LevelSchedule};
use hertzforge_random::RunMode;
use hertzforge_sine::law::{
    LawDescriptor, LawFault, LawValues, SineControlLaw, SineLawFactory, SineLawSetup, TickInputs,
    TickOutputs,
};
use std::sync::Arc;

fn part(id: &str, body: PartBody) -> Part {
    Part {
        id: id.into(),
        name: id.into(),
        enabled: true,
        window: PartWindow::default(),
        body,
    }
}

fn dwell(hz: f64) -> PartSchedule {
    PartSchedule::Fixed {
        hz,
        for_s: None,
        for_cycles: None,
    }
}

fn sweep() -> PartSchedule {
    PartSchedule::Sweep {
        lo_hz: 200.0,
        hi_hz: 280.0,
        start_hz: 200.0,
        direction: Direction::Up,
        law: SweepLaw::LinearHzPerS,
        rate: 4.0,
        target: PartSweepTarget::Duration { s: f64::INFINITY },
    }
}

/// The service's grid for its simulated runs, a floor of `floor` g^2/Hz from
/// 20 to 1000 Hz, one tone of `g` peak, manual at -6 dB.
fn cfg(floor: f64, schedule: PartSchedule, g: f64) -> MixedConfig {
    let mut t = BreakpointTable::new("floor");
    t.push(BreakpointRow::psd(20.0, floor));
    t.push(BreakpointRow::psd(1000.0, floor));
    MixedConfig {
        df_hz: 0.5,
        lines: 1961,
        nfft: Some(8192),
        blocks_per_window: 2,
        control_dof: 120.0,
        control: ControlSet::single(0),
        parts: vec![
            part(
                "floor",
                PartBody::Floor(FloorPart {
                    breakpoints: t,
                    alarm_db: (3.0, 3.0),
                    abort_db: (6.0, 6.0),
                }),
            ),
            part(
                "t1",
                PartBody::Tone(TonePart {
                    schedule,
                    amplitude: ToneAmplitude::Constant { g_pk: g },
                    alarm_db: (1.5, 1.5),
                    abort_db: (3.0, 3.0),
                }),
            ),
        ],
        exclusion_hz: 2.0,
        tracking_hz: 0.5,
        schedule: LevelSchedule {
            mode: RunMode::Manual,
            start_db: -6.0,
            table_db: vec![-6.0, 0.0],
            dwell_s: vec![0.0],
        },
        full_level_duration_s: None,
        level_ramp_s: 1.0,
        shutdown_ramp_s: 1.0,
        part_ramp_s: 1.0,
        floor_settle_s: 10.0,
        soft_start_db: 9.0,
        soft_start_relax_db: 6.0,
        shaker: None,
        moving_mass: Default::default(),
        crest_factor: 3.0,
        identification: MixedIdentification::default(),
        limits: MixedLimits {
            drive_vpk_ceiling: 5.0,
            drive_vpk_abort: Some(8.0),
            ..MixedLimits::default()
        },
        capture: Default::default(),
        law: MixedLawChoice::default(),
        law_guards: None,
        control_tick_s: Some(0.01),
        metadata: Vec::new(),
    }
}

fn engaged(c: MixedConfig, laws: MixedLawFactories) -> MixedService {
    let sim = SimConfig::new()
        .sample_rate_hz(4096.0)
        .resonances(vec![(200.0, 2.5), (550.0, 2.5)])
        .feedthrough(0.35)
        .noise_level(2e-4)
        .shared_clock(true)
        .seed(0x5A5A_1234)
        .build();
    let mut s = MixedService::new(
        c,
        table(),
        Box::new(sim),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        1,
        None,
    )
    .unwrap()
    .with_laws(laws)
    .with_block_frames(128);
    s.arm().unwrap();
    s.run_sysid(0).unwrap();
    assert!(s.coherence_gate().unwrap());
    s.proceed_after_review().unwrap();
    while s.live_status().phase != MixedPhase::Running {
        assert!(s.run_chunk().unwrap(), "{:?}", s.end_reason());
    }
    s
}

/// Ten minutes from the parts coming in: whether an alarm was ever set, the
/// worst judged error, and the end.
fn ten_minutes(schedule: PartSchedule) -> (bool, f64, Option<String>, f64) {
    let c = cfg(5e-4, schedule, 0.1);
    let judge = predict(&c).unwrap().tone_judging[0].judge_s;
    let mut s = engaged(c, MixedLawFactories::builtin());
    let (mut alarm, mut worst) = (false, 0.0f64);
    for _ in 0..2400 {
        if !s.run_chunk().unwrap() {
            break;
        }
        let st = s.live_status();
        let r = st.parts.iter().find(|r| r.kind == PartKind::Tone).unwrap();
        alarm |= r.alarm;
        worst = worst.max(r.judged_error_db.unwrap_or(0.0).abs());
    }
    (alarm, worst, s.end_reason(), judge)
}

#[test]
fn a_small_tone_dwelling_runs_ten_minutes_with_no_alarm_and_no_trip() {
    let (alarm, worst, end, judge) = ten_minutes(dwell(240.0));
    eprintln!(
        "dwell: judged over {judge:.1} s, worst judged {worst:.2} dB, alarm {alarm}, end {end:?}"
    );
    assert!(
        end.is_none() && !alarm,
        "{end:?} alarm {alarm} worst {worst}"
    );
}

#[test]
fn a_small_tone_sweeping_runs_ten_minutes_with_no_alarm_and_no_trip() {
    let (alarm, worst, end, judge) = ten_minutes(sweep());
    eprintln!(
        "sweep: judged over {judge:.1} s, worst judged {worst:.2} dB, alarm {alarm}, end {end:?}"
    );
    assert!(
        end.is_none() && !alarm,
        "{end:?} alarm {alarm} worst {worst}"
    );
}

/// A tone law that plays the identification's prediction 4 dB low.
struct Low;
impl Low {
    fn drive(i: &TickInputs<'_>) -> f64 {
        10f64.powf((i.prediction_db.unwrap_or(-40.0) - 4.0) / 20.0)
    }
}
impl SineControlLaw for Low {
    fn name(&self) -> &str {
        "low"
    }
    fn engage(&mut self, i: &TickInputs<'_>, _: &mut LawValues) -> Result<TickOutputs, LawFault> {
        Ok(TickOutputs {
            drive: Low::drive(i),
            faulted: false,
        })
    }
    fn tick(&mut self, i: &TickInputs<'_>, _: &mut LawValues) -> Result<TickOutputs, LawFault> {
        Ok(TickOutputs {
            drive: Low::drive(i),
            faulted: false,
        })
    }
    fn adjust(&mut self, _: &str, _: f64) -> Result<(), LawFault> {
        Ok(())
    }
}
struct LowFactory;
impl SineLawFactory for LowFactory {
    fn build(&self, _: &SineLawSetup<'_>) -> Result<Box<dyn SineControlLaw + Send>, LawFault> {
        Ok(Box::new(Low))
    }
    fn describe(&self) -> LawDescriptor {
        LawDescriptor {
            id: "low".into(),
            name: "low".into(),
            version: "0".into(),
            sha256: None,
            params: Vec::new(),
            diagnostics: Vec::new(),
            live: Vec::new(),
        }
    }
}

/// Seconds from the tone's first usable reading to its abort, with the tone
/// played 4 dB low; and the trip's error.
fn abort_time(c: MixedConfig) -> (f64, f64) {
    let mut laws = MixedLawFactories::builtin();
    laws.tones = Arc::new(LowFactory);
    let mut s = engaged(c, laws);
    let mut first: Option<f64> = None;
    for _ in 0..400 {
        let alive = s.run_chunk().unwrap();
        let st = s.live_status();
        let r = st.parts.iter().find(|r| r.kind == PartKind::Tone).unwrap();
        if first.is_none() && r.error_db.is_some() {
            first = Some(st.t_s);
        }
        if !alive {
            break;
        }
    }
    let t = s.trip().cloned().expect("tripped");
    let MixedTripKind::ToneAbort { error_db, .. } = t.kind else {
        panic!("{:?}", s.end_reason())
    };
    (t.t_s - first.expect("a reading"), error_db)
}

#[test]
fn a_small_tone_played_4_db_low_still_ends_in_its_abort() {
    let c = cfg(5e-4, dwell(240.0), 0.1);
    let judge = predict(&c).unwrap().tone_judging[0].clone();
    let (after, e) = abort_time(c);
    eprintln!(
        "small tone 4 dB low: judged over {:.1} s (sigma {:.2} dB, widened {}), abort {e:+.1} dB \
         {after:.2} s after its first reading",
        judge.judge_s, judge.sigma_db, judge.widened
    );
    assert!(e < -3.0);
    // Its judged time plus the debounce.
    assert!(after <= judge.judge_s + 0.5, "{after}");
}

#[test]
fn a_large_tone_played_4_db_low_aborts_as_fast_as_before() {
    let c = cfg(5e-4, dwell(240.0), 1.0);
    let judge = predict(&c).unwrap().tone_judging[0].clone();
    assert!(
        !judge.widened && (judge.judge_s - 2.0).abs() < 1e-9,
        "{judge:?}"
    );
    let (after, e) = abort_time(c);
    eprintln!("large tone 4 dB low: judged over {:.1} s, abort {e:+.1} dB {after:.2} s after its first reading", judge.judge_s);
    // Before tone judging the same run aborted 2.25 s after the first reading.
    assert!(e < -3.0 && after <= 2.25 + 1e-9, "{after}");
}
