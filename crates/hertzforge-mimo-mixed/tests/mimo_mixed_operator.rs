//! The operator's verbs on the simulator: the manual mode reaches full level
//! by the level verb and completes, a hold stops the schedule's clocks, and
//! an override takes a failed gate or a prediction over a ceiling on
//! without passing a refusal that protects the hardware.

mod common;
mod service_common;

use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_mimo_mixed::service::*;
use hertzforge_random::RunMode;
use service_common::*;

const S: SineStructure = SineStructure::Linear4x4;

/// One dwelling tone on the floor, held `full_s` at full level.
fn one_tone(full_s: f64) -> hertzforge_mimo_mixed::config::MimoMixedConfig {
    run_config(S, 0.5, vec![tone("t", fixed(240.0), 1.0)], full_s)
}

/// Run chunks until `t_s` or the end; whether the run still runs.
fn run_to(svc: &mut MimoMixedService, t_s: f64) -> bool {
    while svc.status().t_s < t_s {
        if !svc.run_chunk().expect("chunk") {
            return false;
        }
    }
    true
}

/// Requirement: in manual mode the schedule never climbs on its own; the
/// level verb takes the run to full level, the time at full level counts
/// from there, and the run completes after the stated duration.
#[test]
fn mimo_mixed_manual_mode_reaches_full_level_by_the_level_verb_and_completes() {
    let mut cfg = one_tone(3.0);
    cfg.schedule.mode = RunMode::Manual;
    let mut svc = service_unhurried(S, cfg);
    assert_eq!(svc.run_mode(), RunMode::Manual);
    engage(&mut svc);
    assert!(
        run_until(&mut svc, MimoMixedPhase::Schedule, 60.0),
        "{:?}",
        svc.end()
    );
    // Well past the 2 s dwell: no climb.
    let t = svc.status().t_s + 6.0;
    assert!(run_to(&mut svc, t), "{:?}", svc.end());
    assert_eq!(svc.phase(), MimoMixedPhase::Schedule);
    assert!(
        (svc.status().level_db + 6.0).abs() < 1e-9,
        "{}",
        svc.status().level_db
    );
    assert_eq!(svc.set_level_db(0.0).expect("level"), 0.0);
    let t = svc.status().t_s + 1.0;
    assert!(run_to(&mut svc, t), "{:?}", svc.end());
    assert_eq!(svc.phase(), MimoMixedPhase::FullLevel);
    let at_full = svc.status().t_s;
    while svc.run_chunk().expect("chunk") {}
    let end = svc.end().expect("end");
    assert_eq!(end.name, "completed", "{end:?}");
    let took = svc.status().t_s - at_full;
    // 3 s at full level plus the level ramp, then the stop ramp; never
    // before the stated time.
    assert!((3.0..6.0).contains(&took), "{took:.2} s at full level");
}

/// Requirement: a level verb in manual mode clamps to the schedule's span
/// (no lower than its lowest rung or start, no higher than its highest);
/// a step down from full level returns to the schedule and the time at
/// full level starts over.
#[test]
fn mimo_mixed_manual_level_is_clamped_and_a_step_down_leaves_full_level() {
    let mut cfg = one_tone(30.0);
    cfg.schedule.mode = RunMode::Manual;
    let mut svc = service_unhurried(S, cfg);
    engage(&mut svc);
    assert!(
        run_until(&mut svc, MimoMixedPhase::Schedule, 60.0),
        "{:?}",
        svc.end()
    );
    assert_eq!(svc.set_level_db(-40.0).expect("level"), -6.0);
    assert_eq!(svc.set_level_db(6.0).expect("level"), 0.0);
    let t = svc.status().t_s + 1.0;
    assert!(run_to(&mut svc, t), "{:?}", svc.end());
    assert_eq!(svc.phase(), MimoMixedPhase::FullLevel);
    svc.step_level(-3.0).expect("step");
    let t = svc.status().t_s + 0.5;
    assert!(run_to(&mut svc, t), "{:?}", svc.end());
    assert_eq!(svc.phase(), MimoMixedPhase::Schedule);
    svc.stop().expect("stop");
    while svc.run_chunk().expect("chunk") {}
    assert_eq!(svc.end().map(|e| e.name), Some("stopped"));
}

/// Requirement: switching to manual mid-schedule holds the level past the
/// dwell; switching back to automatic restarts the dwell at the present
/// level and climbs to the rung above it.
#[test]
fn mimo_mixed_mode_switches_hold_and_restart_the_climb() {
    let mut svc = service_unhurried(S, one_tone(3.0));
    engage(&mut svc);
    assert!(
        run_until(&mut svc, MimoMixedPhase::Schedule, 60.0),
        "{:?}",
        svc.end()
    );
    svc.set_run_mode(RunMode::Manual).expect("manual");
    let t = svc.status().t_s + 5.0;
    assert!(run_to(&mut svc, t), "{:?}", svc.end());
    assert!((svc.status().level_db + 6.0).abs() < 1e-9);
    svc.set_run_mode(RunMode::Auto).expect("auto");
    let t0 = svc.status().t_s;
    // The dwell is 2 s plus the 0.5 s level ramp, counted from the switch.
    assert!(run_to(&mut svc, t0 + 2.0), "{:?}", svc.end());
    assert!((svc.status().level_db + 6.0).abs() < 1e-9);
    assert!(run_to(&mut svc, t0 + 3.0), "{:?}", svc.end());
    assert!(
        (svc.status().level_db + 3.0).abs() < 1e-9,
        "{}",
        svc.status().level_db
    );
    while svc.run_chunk().expect("chunk") {}
    assert_eq!(svc.end().map(|e| e.name), Some("completed"));
    assert!(svc.set_run_mode(RunMode::Manual).is_err(), "after the end");
}

/// Requirement: a hold stops the schedule's dwell and the time at full
/// level: a run held for longer than its dwell does not climb, and one
/// held at full level does not complete, until resumed.
#[test]
fn mimo_mixed_a_hold_stops_the_dwell_and_the_time_at_full_level() {
    let mut svc = service_unhurried(S, one_tone(2.0));
    engage(&mut svc);
    assert!(
        run_until(&mut svc, MimoMixedPhase::Schedule, 60.0),
        "{:?}",
        svc.end()
    );
    svc.hold().expect("hold");
    let t = svc.status().t_s + 5.0;
    assert!(run_to(&mut svc, t), "{:?}", svc.end());
    assert!(
        (svc.status().level_db + 6.0).abs() < 1e-9,
        "climbed while held: {}",
        svc.status().level_db
    );
    svc.resume().expect("resume");
    let t = svc.status().t_s + 20.0;
    assert!(
        run_until(&mut svc, MimoMixedPhase::FullLevel, t),
        "{:?}",
        svc.end()
    );
    svc.hold().expect("hold");
    let t = svc.status().t_s + 6.0;
    assert!(run_to(&mut svc, t), "completed while held: {:?}", svc.end());
    svc.resume().expect("resume");
    while svc.run_chunk().expect("chunk") {}
    assert_eq!(svc.end().map(|e| e.name), Some("completed"));
}

/// Requirement: the status gives the time at the level in force and the time
/// left on it as the schedule counts them: none before the schedule runs; on
/// a rung in automatic, the two summing to the dwell and the level ramp; in
/// manual under full level, no time left (the level waits for the operator);
/// at full level, the full-level time and the ramp; a hold stops both.
#[test]
fn mimo_mixed_the_status_gives_the_time_at_the_level_and_the_time_left() {
    let mut svc = service_unhurried(S, one_tone(3.0));
    engage(&mut svc);
    let st = svc.status();
    assert_eq!((st.at_level_s, st.remaining_at_level_s), (None, None));
    assert!(
        run_until(&mut svc, MimoMixedPhase::Schedule, 60.0),
        "{:?}",
        svc.end()
    );
    let t = svc.status().t_s + 1.0;
    assert!(run_to(&mut svc, t), "{:?}", svc.end());
    let st = svc.status();
    let (at, left) = (st.at_level_s.expect("at"), st.remaining_at_level_s.expect("left"));
    assert!((0.9..1.6).contains(&at), "{at}");
    // The 2 s dwell and the 0.5 s level ramp.
    assert!((at + left - 2.5).abs() < 1e-9, "{at} + {left}");
    svc.hold().expect("hold");
    let t = svc.status().t_s + 3.0;
    assert!(run_to(&mut svc, t), "{:?}", svc.end());
    let held = svc.status().at_level_s.expect("at");
    assert!((held - at).abs() < 0.3, "the clock ran while held: {at} then {held}");
    svc.set_run_mode(RunMode::Manual).expect("manual");
    let st = svc.status();
    assert!(st.at_level_s.is_some() && st.remaining_at_level_s.is_none(), "{st:?}");
    svc.resume().expect("resume");
    svc.set_level_db(0.0).expect("level");
    let t = svc.status().t_s + 1.0;
    assert!(run_to(&mut svc, t), "{:?}", svc.end());
    assert_eq!(svc.phase(), MimoMixedPhase::FullLevel);
    let st = svc.status();
    let (at, left) = (st.at_level_s.expect("at"), st.remaining_at_level_s.expect("left"));
    // The 3 s at full level and the 0.5 s level ramp.
    assert!((at + left - 3.5).abs() < 1e-9, "{at} + {left}");
    svc.abort("done").expect("abort");
    while svc.run_chunk().expect("chunk") {}
}

/// Requirement: a failed coherence gate holds the run: proceed is refused,
/// the operator's override in words takes it to the review, and an
/// override with nothing to override is refused `override_not_needed`.
#[test]
fn mimo_mixed_a_failed_gate_holds_until_overridden() {
    let mut cfg = one_tone(2.0);
    cfg.gates.coherence_threshold = 0.999_999_999;
    let mut svc = service_unhurried(S, cfg);
    svc.arm().expect("arm");
    let id = svc.identify().expect("identify");
    println!("coherence {:?}", id.band_coherence);
    assert!(!svc.coherence_gate().expect("gate"), "the gate should fail");
    assert_eq!(svc.state(), MimoMixedState::Identified);
    assert!(svc.proceed().is_err(), "proceed past a failed gate");
    assert_eq!(
        svc.override_prediction("go").map_err(|e| e.name()),
        Err("override_not_needed")
    );
    svc.override_gate("the plant's coherence is known low here")
        .expect("override");
    assert_eq!(svc.state(), MimoMixedState::Review);
    assert_eq!(
        svc.gate_override(),
        Some("the plant's coherence is known low here")
    );
    assert_eq!(
        svc.override_gate("again").map_err(|e| e.name()),
        Err("override_not_needed")
    );
    svc.proceed().expect("proceed");
    assert_eq!(svc.state(), MimoMixedState::Running);
    assert!(svc
        .events()
        .iter()
        .any(|e| e.contains("Coherence gate overridden")));
    svc.abort("done").expect("abort");
    while svc.run_chunk().expect("chunk") {}
}

/// Requirement: a prediction that puts a drive's random part over its rms
/// ceiling holds the run (`prediction_exceeded`); the operator's override
/// lets it engage, and the rms abort above the ceiling still holds the
/// drive: no drive's random part plays past it.
#[test]
fn mimo_mixed_a_prediction_over_a_ceiling_holds_until_overridden() {
    let mut cfg = one_tone(5.0);
    let mut svc = service_unhurried(S, cfg.clone());
    svc.arm().expect("arm");
    svc.identify().expect("identify");
    assert!(svc.coherence_gate().expect("gate"));
    let (pred, _) = svc.predict_setup().expect("prediction");
    let rms: Vec<f64> = pred.drives.iter().map(|d| d.random_rms_v).collect();
    println!("predicted random rms {rms:.4?}");
    drop(svc);
    // Every rms ceiling at 0.7 of the predicted random rms: over the
    // ceiling by 3 dB, and under the rms abort's 1 dB above it only if the
    // run holds the drive there.
    for (d, r) in cfg.drives.iter_mut().zip(&rms) {
        d.ceiling_vrms = 0.7 * r;
    }
    let mut svc = service_unhurried(S, cfg.clone());
    svc.arm().expect("arm");
    svc.identify().expect("identify");
    assert!(svc.coherence_gate().expect("gate"));
    let e = svc.proceed().expect_err("the prediction holds");
    assert_eq!(e.name(), "prediction_exceeded", "{e}");
    assert_eq!(svc.state(), MimoMixedState::Review);
    svc.override_prediction("the ceilings are deliberately low for this check")
        .expect("override");
    svc.proceed().expect("proceed");
    assert_eq!(svc.state(), MimoMixedState::Running);
    let mut worst = 0.0f64;
    loop {
        let st = svc.status();
        for (d, c) in st.drives.iter().zip(&cfg.drives) {
            worst = worst.max(d.random_rms_v / c.to_spec().abort_level_vrms());
        }
        if !svc.run_chunk().expect("chunk") || st.t_s > 60.0 {
            break;
        }
    }
    let end = svc.end().map(|e| e.name);
    println!("end {end:?}; worst random rms over its abort {worst:.3}");
    // Either the rms abort ended the run, or no drive's random part
    // reached it.
    assert!(
        end == Some("drive_vrms") || worst < 1.0,
        "a drive's random part played past its abort: {worst:.3}, end {end:?}"
    );
}

/// Requirement: an override never passes the budget forecast. With every
/// peak ceiling under the predicted summed peak the prediction is over a
/// ceiling, the override is taken, and the proceed is still refused
/// `budget_cannot_fit`; the run never starts.
#[test]
fn mimo_mixed_an_override_does_not_pass_the_budget_forecast() {
    let mut cfg = one_tone(5.0);
    let mut svc = service_unhurried(S, cfg.clone());
    svc.arm().expect("arm");
    svc.identify().expect("identify");
    assert!(svc.coherence_gate().expect("gate"));
    let (pred, _) = svc.predict_setup().expect("prediction");
    drop(svc);
    for (d, p) in cfg.drives.iter_mut().zip(&pred.drives) {
        d.peak_v = 0.8 * p.peak_v;
        d.abort_vpk = None;
    }
    let mut svc = service_unhurried(S, cfg);
    svc.arm().expect("arm");
    svc.identify().expect("identify");
    assert!(svc.coherence_gate().expect("gate"));
    assert_eq!(
        svc.predict_setup().map(|_| ()).map_err(|e| e.name()),
        Err("budget_cannot_fit")
    );
    let e = svc.proceed().expect_err("refused");
    assert_eq!(e.name(), "budget_cannot_fit", "{e}");
    assert_eq!(
        svc.override_prediction("go anyway").map_err(|e| e.name()),
        Err("budget_cannot_fit")
    );
    assert_eq!(svc.prediction_override(), None);
    let e = svc.proceed().expect_err("still refused");
    assert_eq!(e.name(), "budget_cannot_fit", "{e}");
    assert_ne!(svc.state(), MimoMixedState::Running);
    assert!(svc.drive_is_zero());
}
