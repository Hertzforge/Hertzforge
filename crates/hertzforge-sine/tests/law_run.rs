//! The run's law as the operator and the records see it: live changes and
//! their refusals, the law's records in the capture folder, the report line.
//!
//! Falsified once each (session log 2026-11-08, Sine core): the range check
//! taken out of `SineService::adjust` reds
//! `a_live_change_is_checked_by_key_liveness_and_range_then_by_the_law`; the
//! records drain taken out of `run_batches` reds
//! `every_call_is_recorded_in_the_capture_folder`.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_hal_sim::SimConfig;
use hertzforge_sine::law::servo_blend::{ServoBlendLaw, ServoBlendParams};
use hertzforge_sine::law::{
    LawDescriptor, LawFault, LawGuards, LawValues, LiveParam, ParamSetting, SineControlLaw,
    SineLawFactory, SineLawSetup, TickInputs, TickOutputs,
};
use hertzforge_sine::service::config::{
    CaptureConfig, ColaConfig, ControlChannel, ControlSet, SineMode, SineTestConfig,
};
use hertzforge_sine::service::law_records::LawRecord;
use hertzforge_sine::service::AdjustRefusal;
use hertzforge_sine::{
    CompositeMode, CompressionPreset, DwellFor, DwellPlan, EventKind, FeedforwardConfig,
    LadderConfig, NotchGovernorConfig, ProfileRow, RunState, RungFor, SineProfile, SineService,
};
use std::sync::Arc;

const FS: f64 = 16_384.0;

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut c = Channel::new(1);
    c.role = ChannelRole::Control;
    c.units = Units::AccelG;
    t.channels.push(c);
    let mut d = Channel::new(2);
    d.role = ChannelRole::Drive;
    d.units = Units::VoltageV;
    t.channels.push(d);
    t
}

fn cfg() -> SineTestConfig {
    let mut profile = SineProfile::new("flat");
    profile.push(ProfileRow::accel(60.0, 1.0));
    profile.push(ProfileRow::accel(500.0, 1.0));
    SineTestConfig {
        profile,
        mode: SineMode::Dwell(DwellPlan {
            freq_hz: 220.0,
            for_: DwellFor::Seconds(1.0),
        }),
        control: ControlSet {
            channels: vec![ControlChannel {
                index: 1,
                weight: 1.0,
            }],
            mode: CompositeMode::WeightedAvg,
        },
        limit_profiles: Vec::new(),
        blend_w: 0.5,
        compression: CompressionPreset::Fast,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        feedforward: FeedforwardConfig::default(),
        notch: NotchGovernorConfig::default(),
        ladder: LadderConfig {
            sysid_level_db: -20.0,
            start_db: 0.0,
            table_db: vec![0.0],
            per_rung: RungFor::Seconds(600.0),
            full_for: RungFor::Seconds(600.0),
        },
        sysid_level_db: -20.0,
        coherence_floor: 0.0,
        drive_vpk_ceiling: 5.0,
        drive_vpk_abort: 10.0,
        fault_debounce_ticks: 0,
        kinematic_monitor: false,
        capture: CaptureConfig::default(),
        cola: ColaConfig::default(),
        shaker: None,
        moving_mass: Default::default(),
        review_hold: false,
        control_tick_frames: 164,
        fs_hz: None,
        metadata: Vec::new(),
        law_guards: Some(LawGuards::default()),
        control_tick_s: None,
    }
}

fn data_root(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "hertzforge-sine-law-run-{tag}-{}",
        std::process::id()
    ))
}

fn service(factory: Option<Arc<dyn SineLawFactory>>, tag: &str) -> SineService {
    service_with(cfg(), factory, tag)
}

fn service_with(
    cfg: SineTestConfig,
    factory: Option<Arc<dyn SineLawFactory>>,
    tag: &str,
) -> SineService {
    let sim = SimConfig::new()
        .sample_rate_hz(FS)
        .resonance(250.0, 20.0)
        .noise_level(1e-4)
        .feedthrough(0.35)
        .shared_clock(true)
        .max_output_channels(2)
        .seed(0x51E5_0003);
    let s = SineService::new(
        cfg,
        table(),
        Box::new(sim.build()),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        0x51E5_0003,
        Some(data_root(tag)),
    )
    .unwrap();
    match factory {
        Some(f) => s.with_law_factory(f),
        None => s,
    }
}

fn engage(s: &mut SineService) {
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_sysid(4096).unwrap();
    assert!(s.coherence_gate().unwrap());
    assert_eq!(s.state(), RunState::Engaged);
}

/// The built-in with one more live parameter, `gain`, that it always
/// refuses: the law's own refusal.
struct Stubborn;

impl SineLawFactory for Stubborn {
    fn build(&self, _setup: &SineLawSetup<'_>) -> Result<Box<dyn SineControlLaw + Send>, LawFault> {
        struct L(ServoBlendLaw);
        impl SineControlLaw for L {
            fn name(&self) -> &str {
                "stubborn"
            }
            fn engage(
                &mut self,
                i: &TickInputs<'_>,
                v: &mut LawValues,
            ) -> Result<TickOutputs, LawFault> {
                self.0.engage(i, v)
            }
            fn tick(
                &mut self,
                i: &TickInputs<'_>,
                v: &mut LawValues,
            ) -> Result<TickOutputs, LawFault> {
                self.0.tick(i, v)
            }
            fn adjust(&mut self, key: &str, value: f64) -> Result<(), LawFault> {
                if key == "gain" {
                    return Err(LawFault::Failed("not now".into()));
                }
                self.0.adjust(key, value)
            }
        }
        Ok(Box::new(L(ServoBlendLaw::new(
            &ServoBlendParams::default(),
        )?)))
    }

    fn describe(&self) -> LawDescriptor {
        let mut d = ServoBlendParams::default().descriptor();
        d.id = "stubborn".into();
        d.params.push(("gain".into(), ParamSetting::Number(1.0)));
        d.live.push(LiveParam {
            key: "gain".into(),
            min: 0.0,
            max: 2.0,
        });
        d
    }
}

#[test]
fn a_live_change_is_checked_by_key_liveness_and_range_then_by_the_law() {
    let mut s = service(Some(Arc::new(Stubborn)), "adjust");
    assert_eq!(s.adjust("blend_w", 0.2), Err(AdjustRefusal::NotEngaged));
    engage(&mut s);
    assert_eq!(s.adjust("nope", 0.2), Err(AdjustRefusal::UnknownKey));
    assert_eq!(s.adjust("kp", 0.2), Err(AdjustRefusal::NotLive));
    assert_eq!(
        s.adjust("blend_w", 1.5),
        Err(AdjustRefusal::OutOfRange { min: 0.0, max: 1.0 })
    );
    match s.adjust("gain", 1.0) {
        Err(AdjustRefusal::LawRefused(why)) => assert!(why.contains("not now"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        [
            AdjustRefusal::NotEngaged,
            AdjustRefusal::UnknownKey,
            AdjustRefusal::NotLive,
            AdjustRefusal::OutOfRange { min: 0.0, max: 1.0 },
            AdjustRefusal::LawRefused(String::new()),
        ]
        .map(|r| r.why()),
        [
            "not_engaged",
            "unknown_key",
            "not_live",
            "out_of_range",
            "law_refused"
        ]
    );
    // A change the law takes is in force at once and logged.
    assert_eq!(s.adjust("blend_w", 0.2), Ok(0.2));
    let live = s.live_status().law.unwrap().live;
    assert!(live.contains(&("blend_w".to_string(), 0.2)), "{live:?}");
    assert!(live.contains(&("gain".to_string(), 1.0)), "{live:?}");
    let logged = s.log().events().iter().any(|e| {
        matches!(&e.kind, EventKind::LawAdjust { key, from, to }
            if key == "blend_w" && *from == 0.5 && *to == 0.2)
    });
    assert!(logged, "the change is logged");
    // The old verb is the same change through the step's mailbox.
    s.set_w(0.3).unwrap();
    assert!(s.set_w(3.0).is_err(), "out of range, refused here");
    s.run_batches(200).unwrap();
    let live = s.live_status().law.unwrap().live;
    assert!(live.contains(&("blend_w".to_string(), 0.3)), "{live:?}");
}

#[test]
fn every_call_is_recorded_in_the_capture_folder() {
    // Ten seconds at a 2 ms tick: more calls than the records ring holds, so
    // only a drain between batches keeps every one.
    let mut c = cfg();
    c.mode = SineMode::Dwell(DwellPlan {
        freq_hz: 220.0,
        for_: DwellFor::Seconds(10.0),
    });
    c.control_tick_frames = 33;
    let mut s = service_with(c, None, "records");
    engage(&mut s);
    let dir = s.start_capture("law_records").unwrap();
    while matches!(s.state(), RunState::Engaged | RunState::FullLevel) {
        if !s.run_chunk().unwrap() {
            break;
        }
    }
    assert_eq!(s.state(), RunState::Stopped);
    let law: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("law.json")).unwrap()).unwrap();
    assert_eq!(law["id"], "servo_blend");
    assert_eq!(law["params"]["blend_w"], 0.5);
    assert_eq!(law["guards"]["max_step_db"], 1.0);
    let side: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("law_ticks.json")).unwrap()).unwrap();
    assert_eq!(side["record_bytes"], LawRecord::BYTES);
    let bytes = std::fs::read(dir.join("law_ticks.bin")).unwrap();
    assert_eq!(bytes.len() % LawRecord::BYTES, 0);
    let records = bytes.len() / LawRecord::BYTES;
    // The engage call with them: the first drain came after the capture
    // started.
    assert!(records > 4_096 + 500, "{records} records");
    assert_eq!(records as u64, s.law_tally().calls);
    assert_eq!(s.law_tally().dropped, 0);
    let tally = s.law_tally();
    assert_eq!(tally.misses, 0);
    assert!(tally.worst_call_us > 0.0, "the calls were timed");
    let report = s.report_data();
    let line = report.law.expect("the report's law line");
    assert_eq!(line.line, "Servo blend 0.1.0");
    assert_eq!((line.misses, line.capped_ticks), (0, 0));
    let _ = std::fs::remove_dir_all(data_root("records"));
}
