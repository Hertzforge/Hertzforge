//! Guards that must trip, on the lagged resonant plant of `common`: a hold
//! stops every clock, the summed drive's peak ends the run, a tone's abort
//! names the tone, and a tone law that stops giving a usable output ends the
//! run by the controlled stop.

mod common;
use common::*;
use hertzforge_mixed::{
    parts::{Direction, PartSchedule, PartSweepTarget, SweepLaw},
    MixedLawFactories, MixedOutcome, MixedPhase, MixedState, MixedTripKind, PartKind,
};
use hertzforge_sine::law::{
    LawDescriptor, LawFault, LawValues, SineControlLaw, SineLawFactory, SineLawSetup, TickInputs,
    TickOutputs,
};
use std::sync::Arc;

fn swept() -> PartSchedule {
    PartSchedule::Sweep {
        lo_hz: 150.0,
        hi_hz: 450.0,
        start_hz: 150.0,
        direction: Direction::Up,
        law: SweepLaw::LinearHzPerS,
        rate: 4.0,
        target: PartSweepTarget::Sweeps { n: 2 },
    }
}

fn quick(mut c: hertzforge_mixed::MixedConfig) -> hertzforge_mixed::MixedConfig {
    c.soft_start_db = 0.0;
    c.floor_settle_s = 3.0;
    c
}

#[test]
fn a_hold_stops_every_clock_and_resume_goes_on_from_there() {
    let mut s = engaged(
        quick(config(vec![floor(0.01), tone("s", swept(), 1.0)])),
        DELAY,
    );
    // Hold just as the swept tone has come in, before its reading is usable:
    // it must hold its feed-forward and settle, not catch up at once.
    while s.live_status().phase != MixedPhase::Running {
        assert!(run_for(&mut s, 0.25), "{:?}", s.end_reason());
    }
    s.hold().unwrap();
    run_for(&mut s, 4.0);
    run_for(&mut s, 0.25);
    let a = s.live_status();
    run_for(&mut s, 3.0);
    let b = s.live_status();
    assert!(b.held);
    assert_eq!(a.test_s, b.test_s);
    let f = |st: &hertzforge_mixed::MixedStatus| st.parts[1].freq_hz.unwrap();
    assert_eq!(f(&a), f(&b));
    // Held, the tone still plays under control at its frequency.
    assert!(b.parts[1].drive > 0.0);
    assert!(s.state().is_running(), "{:?}", s.end_reason());
    s.resume().unwrap();
    run_for(&mut s, 1.0);
    let c = s.live_status();
    assert!((f(&c) - f(&b) - 4.0).abs() < 0.5, "{} -> {}", f(&b), f(&c));
}

#[test]
fn the_summed_drive_peak_ends_the_run_naming_no_part() {
    let mut c = quick(config(vec![floor(0.01), tone("t1", fixed(250.0), 2.0)]));
    // The floor alone peaks near 4 V here; the tone takes the sum past 5 V.
    c.limits.drive_vpk_ceiling = 5.0;
    let mut s = engaged(c, DELAY);
    run_for(&mut s, 20.0);
    assert_eq!(s.state(), MixedState::Aborted);
    let t = s.trip().unwrap();
    assert!(matches!(t.kind, MixedTripKind::DriveVpk { .. }), "{t:?}");
    assert_eq!(t.part, None);
    assert_eq!(s.report_data().outcome, Some(MixedOutcome::Aborted));
}

#[test]
fn a_tone_abort_names_the_tone() {
    let mut c = quick(config(vec![floor(0.01), tone("t1", fixed(250.0), 0.5)]));
    if let hertzforge_mixed::PartBody::Tone(t) = &mut c.parts[1].body {
        // A 0.5 g tone reads with 1.2 dB of scatter: a 0.2 dB abort trips.
        t.abort_db = (0.2, 0.2);
        t.alarm_db = (0.1, 0.1);
    }
    let mut s = engaged(c, DELAY);
    run_for(&mut s, 40.0);
    let t = s.trip().expect("tripped");
    assert!(matches!(t.kind, MixedTripKind::ToneAbort { .. }), "{t:?}");
    assert_eq!(t.part.as_deref(), Some("t1"));
    assert!(s.end_reason().unwrap().starts_with("Tone t1 abort"));
    assert_eq!(s.state(), MixedState::Aborted);
}

/// A tone law that answers NaN.
struct Bad;
impl SineControlLaw for Bad {
    fn name(&self) -> &str {
        "bad"
    }
    fn engage(&mut self, i: &TickInputs<'_>, _: &mut LawValues) -> Result<TickOutputs, LawFault> {
        Ok(TickOutputs {
            drive: 10f64.powf(i.prediction_db.unwrap_or(-20.0) / 20.0),
            faulted: false,
        })
    }
    fn tick(&mut self, _: &TickInputs<'_>, _: &mut LawValues) -> Result<TickOutputs, LawFault> {
        Ok(TickOutputs {
            drive: f64::NAN,
            faulted: false,
        })
    }
    fn adjust(&mut self, _: &str, _: f64) -> Result<(), LawFault> {
        Ok(())
    }
}
struct BadFactory;
impl SineLawFactory for BadFactory {
    fn build(&self, _: &SineLawSetup<'_>) -> Result<Box<dyn SineControlLaw + Send>, LawFault> {
        Ok(Box::new(Bad))
    }
    fn describe(&self) -> LawDescriptor {
        LawDescriptor {
            id: "bad".into(),
            name: "bad".into(),
            version: "0".into(),
            sha256: None,
            params: Vec::new(),
            diagnostics: Vec::new(),
            live: Vec::new(),
        }
    }
}

#[test]
fn a_tone_law_fault_ends_the_run_by_the_controlled_stop() {
    let mut laws = MixedLawFactories::builtin();
    laws.tones = Arc::new(BadFactory);
    let c = quick(config(vec![floor(0.01), tone("t1", fixed(250.0), 2.0)]));
    let mut s = engaged_with(c, DELAY, laws);
    let mut saw_ramp = false;
    for _ in 0..120 {
        let alive = run_for(&mut s, 0.25);
        saw_ramp |= s.live_status().phase == MixedPhase::RampingDown;
        if !alive {
            break;
        }
    }
    let t = s.trip().expect("tripped");
    assert!(
        matches!(&t.kind, MixedTripKind::LawFault { reason, .. } if reason == "non_finite"),
        "{t:?}"
    );
    assert_eq!(t.part.as_deref(), Some("t1"));
    assert!(saw_ramp, "the stop was a ramp");
    assert_eq!(s.report_data().outcome, Some(MixedOutcome::LawFault));
    let tally = s.law_status();
    let _ = (tally, PartKind::Tone);
}
