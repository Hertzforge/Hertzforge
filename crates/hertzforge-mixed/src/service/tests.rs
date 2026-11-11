use super::*;
use crate::parts::tests_support::*;
use crate::parts::PartSweepTarget;
use crate::service::status::{MixedPhase, PartKind};
use hertzforge_channel_table::{Channel, ChannelRole, Units};
use hertzforge_hal_sim::SimConfig;

fn table() -> ChannelTable {
    let mut table = ChannelTable::new();
    let mut c = Channel::new(0);
    c.role = ChannelRole::Control;
    c.units = Units::AccelG;
    table.channels.push(c);
    let mut d = Channel::new(1);
    d.role = ChannelRole::Drive;
    d.units = Units::VoltageV;
    table.channels.push(d);
    table
}

fn station(fs: f64) -> Box<dyn Backend> {
    Box::new(SimConfig::new().sample_rate_hz(fs).seed(7).build())
}

fn combined() -> MixedConfig {
    config(vec![
        floor("floor"),
        tone("t1", fixed(100.0)),
        tone(
            "t2",
            sweep(400.0, 800.0, 450.0, PartSweepTarget::Sweeps { n: 2 }),
        ),
        tone("t3", follow("t1", 3.0)),
        band(
            "b1",
            fixed(1200.0),
            crate::parts::BandWidth::Hz { hz: 20.0 },
        ),
    ])
}

fn service(cfg: MixedConfig) -> Result<MixedService, MixedError> {
    MixedService::new(
        cfg,
        table(),
        station(8192.0),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        1,
        None,
    )
}

#[test]
fn a_good_configuration_builds_idle_with_a_row_per_part() {
    let s = service(combined()).unwrap();
    assert_eq!(s.state(), MixedState::Idle);
    assert_eq!(s.sample_rate_hz(), 8192.0);
    let st = s.live_status();
    assert_eq!(st.phase, MixedPhase::Idle);
    let kinds: Vec<_> = st.parts.iter().map(|r| (r.id.as_str(), r.kind)).collect();
    assert_eq!(
        kinds,
        vec![
            ("floor", PartKind::Floor),
            ("t1", PartKind::Tone),
            ("t2", PartKind::Tone),
            ("t3", PartKind::Tone),
            ("b1", PartKind::Narrowband),
        ]
    );
    assert_eq!(
        st.parts[2].freq_hz,
        Some(450.0),
        "a sweep starts at its start"
    );
    assert_eq!(st.parts[3].freq_hz, Some(300.0), "a follower at its ratio");
    assert!(st.floor.rms_target_g > 0.0);
    assert_eq!(s.law_guards(), MixedLawGuards::default());
    assert_eq!(s.laws().tones.describe().id, "servo_blend");
}

#[test]
fn new_refuses_a_configuration_by_name_at_the_station_rate() {
    let e = service(config(vec![tone("t", fixed(100.0))])).unwrap_err();
    assert_eq!(e.name(), "no_floor");
    // The floor's band (to 2000 Hz) is above half of 2048 Hz.
    let e = MixedService::new(
        combined(),
        table(),
        station(2048.0),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        1,
        None,
    )
    .unwrap_err();
    assert_eq!(e.name(), "part_out_of_band");
    // A control channel the table does not hold.
    let mut c = combined();
    c.control = hertzforge_random::ControlSet::single(5);
    assert_eq!(service(c).unwrap_err().name(), "bad_config");
}

#[test]
fn what_is_not_built_is_refused_by_a_typed_error() {
    let mut s = service(combined()).unwrap();
    for (e, what) in [
        (s.arm().unwrap_err(), "arm"),
        (s.run_sysid(0).unwrap_err(), "identify the plant"),
        (s.coherence_gate().unwrap_err(), "judge the identification"),
        (s.proceed_after_review().unwrap_err(), "engage"),
        (s.run_chunk().unwrap_err(), "run"),
        (s.hold().unwrap_err(), "hold"),
        (
            s.set_part_enabled("t1", false).unwrap_err(),
            "switch a part",
        ),
    ] {
        assert_eq!(e, MixedError::NotBuilt(what));
        assert_eq!(e.name(), "not_built");
    }
    assert_eq!(
        s.adjust(LawRole::Tones, "blend_w", 0.5).unwrap_err().why(),
        "not_engaged"
    );
    // Before engage there is nothing to take down: stop and abort succeed.
    s.stop().unwrap();
    s.abort("test").unwrap();
    s.set_run_mode(RunMode::Manual).unwrap();
    assert_eq!(s.run_mode(), RunMode::Manual);
    assert_eq!(s.live_status().run_mode, RunMode::Manual);
    assert!(s.drive_is_zero());
    let r = s.report_data();
    assert_eq!(r.config.parts.len(), 5);
    assert_eq!(r.events.len(), 1);
}

#[test]
fn a_trip_names_its_part() {
    let t = status::MixedTrip {
        kind: status::MixedTripKind::ToneAbort { error_db: 3.4 },
        part: Some("t3".into()),
        t_s: 12.0,
    };
    assert_eq!(t.words(), "Tone t3 abort: +3.4 dB");
    let v = serde_json::to_value(&t).unwrap();
    assert_eq!(v["kind"]["kind"], "tone_abort");
    assert_eq!(v["part"], "t3");
}
