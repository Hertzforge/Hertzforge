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
fn a_run_asks_the_station_for_the_rate_the_setup_states_not_the_stations_highest() {
    // A station that states 8192 Hz in its caps; the setup was written for 4096.
    let s = service(combined()).unwrap();
    assert_eq!(
        s.sample_rate_hz(),
        8192.0,
        "without a request, the caps' rate"
    );
    let s = s.with_requested_rate(4096.0).unwrap();
    assert_eq!(s.sample_rate_hz(), 4096.0, "the rate the run will ask for");
    // The configuration is checked at the requested rate: a band at 3000 Hz
    // fits under 8192 Hz and not under 4096 Hz.
    let high = config(vec![
        floor("floor"),
        band(
            "b1",
            fixed(1200.0),
            crate::parts::BandWidth::Hz { hz: 20.0 },
        ),
    ]);
    assert!(service(high.clone())
        .unwrap()
        .with_requested_rate(4096.0)
        .is_ok());
    assert!(service(high).unwrap().with_requested_rate(1024.0).is_err());
    assert!(service(combined())
        .unwrap()
        .with_requested_rate(0.0)
        .is_err());
    assert!(service(combined())
        .unwrap()
        .with_requested_rate(f64::NAN)
        .is_err());
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
fn every_step_out_of_order_is_refused_by_state() {
    let mut s = service(combined()).unwrap();
    for e in [
        s.run_sysid(0).unwrap_err(),
        s.coherence_gate().unwrap_err(),
        s.proceed_after_review().unwrap_err(),
        s.run_chunk().unwrap_err(),
        s.hold().unwrap_err(),
        s.set_part_enabled("t1", false).unwrap_err(),
        s.set_level_db(-3.0).unwrap_err(),
        s.request_stop().unwrap_err(),
    ] {
        assert_eq!(e.name(), "bad_state", "{e}");
    }
    assert_eq!(s.start_capture("x").unwrap_err().name(), "bad_state");
    assert_eq!(
        s.adjust(LawRole::Tones, "blend_w", 0.5).unwrap_err().why(),
        "not_engaged"
    );
    // The floor cannot be switched off.
    assert_eq!(
        s.set_part_enabled("floor", false).unwrap_err().name(),
        "bad_state"
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
        kind: status::MixedTripKind::ToneAbort {
            error_db: 3.4,
            held_at_vpk: None,
        },
        part: Some("t3".into()),
        t_s: 12.0,
    };
    assert_eq!(t.words(), "Tone t3 abort: +3.4 dB");
    let v = serde_json::to_value(&t).unwrap();
    assert_eq!(v["kind"]["kind"], "tone_abort");
    assert_eq!(v["part"], "t3");
    let f = status::MixedTrip {
        kind: status::MixedTripKind::LawFault {
            role: LawRole::Tones,
            reason: "over_budget".into(),
        },
        part: Some("t1".into()),
        t_s: 1.0,
    };
    assert_eq!(f.words(), "Tone t1 law fault: over_budget");
}

/// D6: a line a swept tone crosses pauses and resumes. One loop after the
/// tone has left it, the line holds close to the degrees of freedom it held
/// before the tone arrived, and its average is the floor's, not the tone's.
#[test]
fn a_line_a_swept_tone_crosses_pauses_and_resumes_with_its_earlier_frames() {
    // The closed-loop tests' plant and grid: 4096 Hz, one mode at 300 Hz,
    // Q 8, 40 frames of delay; 4 Hz lines, 8 frames a loop, 60 DOF.
    let mut flr = floor("floor");
    if let crate::parts::PartBody::Floor(f) = &mut flr.body {
        f.breakpoints = hertzforge_breakpoints::BreakpointTable::new("floor");
        f.breakpoints
            .push(hertzforge_breakpoints::BreakpointRow::psd(40.0, 0.01));
        f.breakpoints
            .push(hertzforge_breakpoints::BreakpointRow::psd(1000.0, 0.01));
    }
    let mut c = config(vec![
        flr,
        tone(
            "t",
            sweep(150.0, 450.0, 150.0, PartSweepTarget::Sweeps { n: 1 }),
        ),
    ]);
    c.df_hz = 4.0;
    c.lines = 256;
    c.nfft = Some(1024);
    c.blocks_per_window = 8;
    c.control_dof = 60.0;
    c.exclusion_hz = 8.0;
    c.part_ramp_s = 1.0;
    c.level_ramp_s = 1.0;
    c.shutdown_ramp_s = 1.0;
    c.soft_start_db = 0.0;
    c.floor_settle_s = 3.0;
    c.schedule.start_db = 0.0;
    c.schedule.table_db = vec![0.0];
    c.identification.sysid_level_db = -12.0;
    c.limits.drive_vpk_ceiling = 20.0;
    if let crate::parts::PartBody::Tone(t) = &mut c.parts[1].body {
        t.amplitude = crate::parts::ToneAmplitude::Constant { g_pk: 1.0 };
    }
    let sim = SimConfig::new()
        .sample_rate_hz(4096.0)
        .resonance(300.0, 8.0)
        .feedthrough(1.0)
        .noise_level(1e-4)
        .transport_delay_samples(40)
        .shared_clock(true)
        .seed(0x4D49_5845)
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
    .with_block_frames(64);
    s.arm().unwrap();
    s.run_sysid(0).unwrap();
    assert!(s.coherence_gate().unwrap());
    s.proceed_after_review().unwrap();
    let k = 50usize; // 200 Hz on 4 Hz lines
                     // One loop: 8 frames of 512-frame hops at 4096 Hz, a second, 4 chunks.
    let loop_chunks = 4;
    // (dof, average) a chunk before the tone covers the line; then its state
    // one loop after the tone has left it.
    let mut before: Option<(f64, f64)> = None;
    let mut since_resume: Option<usize> = None;
    let mut after: Option<(f64, f64)> = None;
    let mut prev = (0.0, f64::NAN);
    for _ in 0..400 {
        assert!(s.run_chunk().unwrap(), "{:?}", s.end_reason());
        let est = &s.rt.as_ref().unwrap().est;
        let paused = est.paused(k);
        if paused && before.is_none() {
            before = Some(prev);
        }
        prev = (est.dof(k), est.sum_line(k));
        if before.is_some() && !paused {
            let n = since_resume.map_or(0, |n| n + 1);
            since_resume = Some(n);
            if n == loop_chunks {
                after = Some((est.dof(k), est.line(k).unwrap()));
                break;
            }
        } else {
            since_resume = None;
        }
    }
    let (dof0, m0) = before.expect("the tone reached the line");
    let (dof1, m1) = after.expect("the tone left the line");
    let shift_db = 10.0 * (m1 / m0).log10();
    eprintln!(
        "line {k} (200 Hz): {dof0:.1} dof before the tone, {dof1:.1} one loop after it left; average moved {shift_db:+.2} dB"
    );
    assert!(dof1 >= 0.9 * dof0, "{dof0} -> {dof1}");
    assert!(shift_db.abs() < 1.0, "{shift_db}");
}
