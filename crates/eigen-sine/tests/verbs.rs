//! The operator verbs: GoTo, SetLevel{Db}, SetRunMode - and the refusals.
//!
//! Every assertion here is about the ECHO or the REFUSAL, never about the
//! physics: the panel renders the SERVICE's state, so a verb whose returned
//! value disagrees with what the scheduler applied is a control that lies, and
//! a verb that no-ops in a mode it does not serve is worse.
//!
//! RED-MUTATIONS, each observed once:
//!
//! * Delete the `manual` early-return from `LadderPlan::advance_if_met`
//!   (`src/scheduler.rs`): `manual_mode_stops_the_ladder_advancing` AND
//!   `the_run_mode_can_be_set_before_the_step_is_built` red, the other seven stay
//!   green. **Observed red 2026-08-08** - the ladder walked -12 -> 0 dB with the
//!   mode set to MANUAL, which is the state the lying `run_manual` doc described
//!   for the life of the module.
//! * Make `SineService::require_sweep` return `Ok(())` unconditionally:
//!   `a_frequency_intent_outside_sweep_refuses_by_name` reds alone. **Observed
//!   red 2026-08-08** - the verb was accepted in dwell mode and the command was
//!   then silently dropped by the scheduler, which is exactly the silent no-op
//!   this file forbids.

use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use eigen_dsp::tracking::BandwidthMode;
use eigen_hal_sim::SimConfig;
use eigen_sine::service::config::{
    CaptureConfig, ColaConfig, ControlChannel, ControlSet, SineMode, SineTestConfig,
};
use eigen_sine::{
    BidirSweepConfig, CompositeMode, CompressionPreset, DwellFor, DwellPlan, FeedforwardConfig,
    LadderConfig, LadderPlan, NotchGovernorConfig, ProfileRow, RunMode, RunState, RungFor,
    SineProfile, SineService, SweepDirection, SweepRate, SweepTarget,
};

const FS: f64 = 16_384.0;
const SEED: u64 = 0x51E5_0001;
const BAND: (f64, f64) = (60.0, 500.0);
/// A rung short enough that AUTO walks the whole table inside a few chunks.
const RUNG_SHORT_S: f64 = 0.05;
/// A rung longer than any test here runs, so the ladder cannot move a level the
/// operator just set and turn an echo assertion into a race.
const RUNG_LONG_S: f64 = 600.0;

fn table_1ctrl() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(ctrl);
    t.channels.push(drv);
    t
}

fn cfg(mode: SineMode, table_db: Vec<f64>, rung_s: f64) -> SineTestConfig {
    let mut profile = SineProfile::new("flat");
    profile.push(ProfileRow::accel(BAND.0, 1.0));
    profile.push(ProfileRow::accel(BAND.1, 1.0));
    let start_db = table_db[0];
    SineTestConfig {
        profile,
        mode,
        control: ControlSet {
            channels: vec![ControlChannel {
                index: 1,
                weight: 1.0,
            }],
            mode: CompositeMode::WeightedAvg,
        },
        limit_profiles: Vec::new(),
        blend_w: 1.0,
        compression: CompressionPreset::Fast,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        feedforward: FeedforwardConfig::default(),
        notch: NotchGovernorConfig::default(),
        ladder: LadderConfig {
            sysid_level_db: -20.0,
            start_db,
            table_db,
            // Seconds rungs so the ladder advances on the TICK CLOCK: a Sweeps
            // rule would need band-edge arrivals, which a dwell never produces
            // and which would make the mode tests measure the sweep instead of
            // the ladder. `rung_s` is the axis: SHORT where the test is about the
            // ladder walking, LONG where the test is about a level the operator
            // set and the ladder must not walk away from underneath it.
            per_rung: RungFor::Seconds(rung_s),
            full_for: RungFor::Seconds(rung_s),
        },
        sysid_level_db: -20.0,
        coherence_floor: 0.0,
        drive_vpk_ceiling: 1e4,
        drive_vpk_abort: 1e5,
        fault_debounce_ticks: 0,
        kinematic_monitor: false,
        capture: CaptureConfig::default(),
        cola: ColaConfig::default(),
        shaker: None,
        moving_mass: Default::default(),
        // The verbs under test live PAST the gate, so the park is off here; the
        // park itself is `review_hold.rs`.
        review_hold: false,
        control_tick_frames: 64,
        fs_hz: None,
        metadata: Vec::new(),
    }
}

fn sweep_mode() -> SineMode {
    SineMode::Sweep(BidirSweepConfig {
        band: BAND,
        rate: SweepRate::LogOctPerMin(1.0),
        start_hz: 100.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(8),
    })
}

fn dwell_mode() -> SineMode {
    SineMode::Dwell(DwellPlan {
        freq_hz: 200.0,
        for_: DwellFor::Seconds(600.0),
    })
}

/// A service driven to Engaged (the state every verb here is issued from).
fn engaged(cfg: SineTestConfig) -> SineService {
    let sim = SimConfig::new()
        .sample_rate_hz(FS)
        .resonance(250.0, 20.0)
        .noise_level(1e-4)
        .feedthrough(0.35)
        .shared_clock(true)
        .max_output_channels(2)
        .seed(SEED);
    let mut s = SineService::new(
        cfg,
        table_1ctrl(),
        Box::new(sim.build()),
        eigen_module_abi::StationClass::EigenSim,
        SEED,
        None,
    )
    .unwrap();
    s.assess_feasibility().expect("no shaker configured");
    s.arm().expect("the Sim declares a shared clock");
    s.run_sysid(4096).expect("sysID");
    assert!(s.coherence_gate().expect("gate"), "floor 0.0 must pass");
    assert_eq!(s.state(), RunState::Engaged);
    s
}

/// Pump chunks until `pred` holds or the budget runs out.
fn pump_until(s: &mut SineService, limit: u32, mut pred: impl FnMut(&SineService) -> bool) -> bool {
    for _ in 0..limit {
        if pred(s) {
            return true;
        }
        match s.run_chunk() {
            Ok(true) => {}
            Ok(false) | Err(_) => break,
        }
    }
    pred(s)
}

// =============================================================================
// GoTo
// =============================================================================

#[test]
fn go_to_echoes_the_clamped_value_and_the_scheduler_lands_on_it() {
    let mut s = engaged(cfg(sweep_mode(), vec![0.0], RUNG_LONG_S));
    // HOLD first. A running sweep moves the tone every tick, so without this the
    // test would be asserting that a GoTo landed AND that the sweep had not yet
    // walked away from it - two things, one of them a race.
    s.hold().expect("hold");
    let echo = s.go_to(300.0).expect("sweep mode serves GoTo");
    assert!(
        (echo - 300.0).abs() < 1e-9,
        "in-band echo is the request: {echo}"
    );
    // The ECHO and the SCHEDULER must be the same number. A clamp that lived only
    // in the oscillator would leave the servo targeting one frequency while the
    // status reported another.
    assert!(
        pump_until(&mut s, 200, |s| (s.status().freq_hz - echo).abs() < 1e-6),
        "the scheduler never reached the echoed {echo} Hz (at {:.3})",
        s.status().freq_hz
    );
}

#[test]
fn go_to_clamps_to_the_band_and_says_so() {
    let mut s = engaged(cfg(sweep_mode(), vec![0.0], RUNG_LONG_S));
    s.hold().expect("hold");
    let high = s.go_to(50_000.0).expect("sweep mode serves GoTo");
    assert!(
        (high - BAND.1).abs() < 1e-9,
        "above-band must clamp to the high edge, echoed {high}"
    );
    assert!(
        pump_until(&mut s, 200, |s| (s.status().freq_hz - BAND.1).abs() < 1e-6),
        "the scheduler must land where the echo said: {:.3}",
        s.status().freq_hz
    );
    let low = s.go_to(0.1).expect("sweep mode serves GoTo");
    assert!(
        (low - BAND.0).abs() < 1e-9,
        "below-band must clamp to the low edge, echoed {low}"
    );
    assert!(
        pump_until(&mut s, 200, |s| (s.status().freq_hz - BAND.0).abs() < 1e-6),
        "at {:.3}",
        s.status().freq_hz
    );
}

/// RED-MUTATION test (see the module doc).
#[test]
fn a_frequency_intent_outside_sweep_refuses_by_name() {
    let s = engaged(cfg(dwell_mode(), vec![0.0], RUNG_LONG_S));
    for (verb, res) in [("go_to", s.go_to(300.0)), ("jog", s.jog(10.0))] {
        let text = res
            .map(|hz| format!("ACCEPTED, echoing {hz}"))
            .expect_err(&format!("{verb} must refuse in dwell mode"))
            .to_string();
        assert!(
            text.contains("dwell"),
            "{verb}'s refusal must NAME the mode: {text}"
        );
        assert!(
            text.contains(verb),
            "{verb}'s refusal must name the verb: {text}"
        );
    }
}

// =============================================================================
// SetLevel{Db} - the nearest-rung rule
// =============================================================================

#[test]
fn nearest_rung_resolves_between_rungs_clamps_outside_and_breaks_ties_low() {
    let table = [-12.0, -6.0, 0.0];
    assert_eq!(LadderPlan::nearest_rung(&table, -6.4), Some((1, -6.0)));
    assert_eq!(LadderPlan::nearest_rung(&table, -2.0), Some((2, 0.0)));
    // OUT of the table is the same arithmetic, not a separate clamp branch.
    assert_eq!(LadderPlan::nearest_rung(&table, -100.0), Some((0, -12.0)));
    assert_eq!(LadderPlan::nearest_rung(&table, 40.0), Some((2, 0.0)));
    // A tie goes DOWN: the safe side of an ambiguous level request.
    assert_eq!(LadderPlan::nearest_rung(&table, -9.0), Some((0, -12.0)));
    assert_eq!(LadderPlan::nearest_rung(&[], 0.0), None);
    assert_eq!(LadderPlan::nearest_rung(&table, f64::NAN), None);
}

#[test]
fn set_level_db_echoes_the_applied_rung_never_the_request() {
    let mut s = engaged(cfg(sweep_mode(), vec![-12.0, -6.0, 0.0], RUNG_LONG_S));
    let applied = s.set_level_db(-5.2).expect("level intent");
    assert!(
        (applied - (-6.0)).abs() < 1e-9,
        "-5.2 dB is nearest the -6 dB rung; echoed {applied}"
    );
    assert!(
        pump_until(&mut s, 400, |s| (s.status().level_db - (-6.0)).abs() < 1e-9),
        "the ladder must land on the echoed rung (at {:.3} dB)",
        s.status().level_db
    );
}

#[test]
fn set_level_db_outside_the_table_lands_on_the_end_rung() {
    let mut s = engaged(cfg(sweep_mode(), vec![-12.0, -6.0, 0.0], RUNG_LONG_S));
    // The run starts at -12 dB, so the UP clamp is the one that has to MOVE
    // something: a down-clamp from the bottom rung would pass whether or not the
    // verb did anything at all.
    let up = s.set_level_db(40.0).expect("level intent");
    assert!(
        (up - 0.0).abs() < 1e-9,
        "above the table must land on 0 dB, echoed {up}"
    );
    assert!(
        pump_until(&mut s, 400, |s| (s.status().level_db - 0.0).abs() < 1e-9),
        "at {:.3} dB",
        s.status().level_db
    );
    let down = s.set_level_db(-99.0).expect("level intent");
    assert!(
        (down - (-12.0)).abs() < 1e-9,
        "below the table must land on -12 dB, echoed {down}"
    );
    assert!(
        pump_until(&mut s, 400, |s| (s.status().level_db - (-12.0)).abs()
            < 1e-9),
        "at {:.3} dB",
        s.status().level_db
    );
}

// =============================================================================
// SetRunMode
// =============================================================================

/// RED-MUTATION test (see the module doc). The ladder must NOT walk itself in
/// MANUAL - which is what `run_manual` did for the life of the module, because
/// it was literally `run_auto` and `LadderPlan` had no manual flag at all.
#[test]
fn manual_mode_stops_the_ladder_advancing() {
    let mut c = cfg(sweep_mode(), vec![-12.0, -6.0, 0.0], RUNG_SHORT_S);
    c.ladder.start_db = -12.0;
    let mut s = engaged(c);
    s.set_run_mode(RunMode::Manual).expect("mode intent");
    // 0.05 s rungs against a 64-frame control tick at 16 384 Hz: ~13 ticks per
    // rung, and a chunk is thousands of ticks. In AUTO this many chunks walks the
    // whole table several times over.
    for _ in 0..20 {
        if s.run_chunk().is_err() {
            break;
        }
    }
    assert_eq!(
        s.status().level_db,
        -12.0,
        "MANUAL must hold the start rung; the ladder advanced itself"
    );
    assert!(
        !matches!(s.state(), RunState::FullLevel),
        "MANUAL must not complete the ladder"
    );
}

/// The other half: the same fixture in AUTO must reach full level, or the test
/// above proves nothing (a ladder that never advanced for an unrelated reason
/// would pass it).
#[test]
fn auto_mode_still_walks_the_ladder_to_full() {
    let mut c = cfg(sweep_mode(), vec![-12.0, -6.0, 0.0], RUNG_SHORT_S);
    c.ladder.start_db = -12.0;
    let mut s = engaged(c);
    assert_eq!(s.run_mode(), RunMode::Auto);
    let reached = pump_until(&mut s, 20, |s| (s.status().level_db - 0.0).abs() < 1e-9);
    assert!(
        reached,
        "AUTO must climb the table; stuck at {:.3} dB",
        s.status().level_db
    );
}

/// The mode is selectable BEFORE the step exists. A MANUAL run is chosen at
/// Start and the step is not built until the gate releases, so a mode that could
/// only be posted into the mailbox would arrive after the first rung rule had
/// already been evaluated.
#[test]
fn the_run_mode_can_be_set_before_the_step_is_built() {
    let sim = SimConfig::new()
        .sample_rate_hz(FS)
        .resonance(250.0, 20.0)
        .noise_level(1e-4)
        .feedthrough(0.35)
        .shared_clock(true)
        .max_output_channels(2)
        .seed(SEED);
    let mut c = cfg(sweep_mode(), vec![-12.0, -6.0, 0.0], RUNG_SHORT_S);
    c.ladder.start_db = -12.0;
    let mut s = SineService::new(
        c,
        table_1ctrl(),
        Box::new(sim.build()),
        eigen_module_abi::StationClass::EigenSim,
        SEED,
        None,
    )
    .unwrap();
    s.set_run_mode(RunMode::Manual)
        .expect("legal with no step yet");
    assert_eq!(s.run_mode(), RunMode::Manual);
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_sysid(4096).unwrap();
    assert!(s.coherence_gate().unwrap());
    for _ in 0..20 {
        if s.run_chunk().is_err() {
            break;
        }
    }
    assert_eq!(
        s.status().level_db,
        -12.0,
        "the mode chosen before the build must reach the ladder"
    );
}
