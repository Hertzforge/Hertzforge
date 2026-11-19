//! A hold with a follower: a swept tone and a follower at twice its frequency
//! are held for five seconds and resumed, on the lagged resonant plant of
//! `common`. While held both stand still under control; after the resume both
//! move on from where they stopped and stay within their alarm bands.

mod common;
use common::*;
use hertzforge_mixed::{
    parts::{Direction, PartSchedule, PartSweepTarget, SweepLaw},
    MixedPhase, MixedService, PartKind,
};

fn cfg() -> hertzforge_mixed::MixedConfig {
    let sweep = PartSchedule::Sweep {
        lo_hz: 150.0,
        hi_hz: 400.0,
        start_hz: 150.0,
        direction: Direction::Up,
        law: SweepLaw::LinearHzPerS,
        rate: 1.0,
        target: PartSweepTarget::Duration { s: f64::INFINITY },
    };
    let follow = PartSchedule::Follow {
        of: "lead".into(),
        ratio: 2.0,
    };
    let mut c = config(vec![
        floor(0.001),
        tone("lead", sweep, 2.0),
        tone("fol", follow, 2.0),
    ]);
    c.soft_start_db = 0.0;
    c.floor_settle_s = 3.0;
    c
}

/// Each tone's (frequency, error) after `secs` more, and the worst error
/// magnitude over that span.
fn run(s: &mut MixedService, secs: f64) -> (Vec<(f64, Option<f64>)>, f64) {
    let mut worst = 0.0f64;
    for _ in 0..(secs / 0.25) as usize {
        assert!(run_for(s, 0.25), "{:?}", s.end_reason());
        for r in s
            .live_status()
            .parts
            .iter()
            .filter(|r| r.kind == PartKind::Tone)
        {
            worst = worst.max(r.error_db.unwrap_or(0.0).abs());
        }
    }
    let st = s.live_status();
    let now = st
        .parts
        .iter()
        .filter(|r| r.kind == PartKind::Tone)
        .map(|r| (r.freq_hz.unwrap_or(0.0), r.error_db))
        .collect();
    (now, worst)
}

#[test]
fn a_held_and_resumed_follower_comes_back_within_its_alarm() {
    let mut s = engaged(cfg(), DELAY);
    while s.live_status().phase != MixedPhase::Running {
        assert!(run_for(&mut s, 0.25), "{:?}", s.end_reason());
    }
    run(&mut s, 12.0);
    s.hold().unwrap();
    let (held_at, _) = run(&mut s, 0.25);
    let (still, worst_held) = run(&mut s, 5.0);
    s.resume().unwrap();
    let (after, worst_after) = run(&mut s, 15.0);
    eprintln!(
        "held at {held_at:?}, still {still:?} (worst {worst_held:.2} dB); \
         15 s after the resume {after:?} (worst {worst_after:.2} dB)"
    );
    // Held, neither moves and the follower stays at twice its leader.
    assert_eq!(held_at[0].0, still[0].0);
    assert!((still[1].0 - 2.0 * still[0].0).abs() < 0.01);
    // Resumed, both move on and every reading stays within the 1.5 dB alarm.
    assert!(after[0].0 > still[0].0 + 10.0);
    assert!((after[1].0 - 2.0 * after[0].0).abs() < 0.01);
    assert!(
        worst_held < 1.5 && worst_after < 1.5,
        "{worst_held} {worst_after}"
    );
}
