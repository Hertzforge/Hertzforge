//! The level ladder on the lagged resonant plant of `common`: automatic
//! advance moving every part together, from a start level that is not a rung,
//! with a hold stopping the rung's clock as well as every part clock.

mod common;
use common::*;
use hertzforge_mixed::{MixedPhase, MixedService, MixedState, PartKind};

fn ladder_cfg() -> hertzforge_mixed::MixedConfig {
    let mut c = config(vec![floor(0.01), tone("t1", fixed(250.0), 2.0)]);
    c.soft_start_db = 0.0;
    c.floor_settle_s = 3.0;
    c.schedule.table_db = vec![-12.0, -6.0, 0.0];
    c.schedule.start_db = -9.0;
    c.schedule.dwell_s = vec![4.0];
    c.full_level_duration_s = Some(3.0);
    c
}

/// The level, the tone's target and the floor's target, every half second.
fn trace(s: &mut MixedService, secs: f64) -> Vec<(f64, f64, f64, f64, bool)> {
    let mut out = Vec::new();
    for _ in 0..(secs / 0.5) as usize {
        let alive = run_for(s, 0.5);
        let st = s.live_status();
        let tone = st.parts.iter().find(|r| r.kind == PartKind::Tone).unwrap();
        out.push((
            st.t_s,
            st.level_db,
            tone.target,
            st.floor.rms_target_g,
            st.held,
        ));
        if !alive {
            break;
        }
    }
    out
}

#[test]
fn the_ladder_starts_at_a_level_off_the_table_and_climbs_every_part_together() {
    let mut s = engaged(ladder_cfg(), DELAY);
    let tr = trace(&mut s, 40.0);
    // The first level the parts run at is the start, -9 dB, not the table's -12.
    let first = tr.iter().find(|x| x.2 > 0.0).unwrap();
    assert!((first.1 + 9.0).abs() < 0.05, "{first:?}");
    // Never below the start on the way up (the stop's ramp-out comes after).
    let up = tr
        .iter()
        .position(|x| x.1.abs() < 0.05)
        .expect("reaches full");
    assert!(
        tr[..up].iter().all(|x| x.1 > -9.05),
        "never below the start"
    );
    // It visits -6 and then 0, and every part moves by the same dB: the
    // tone's target and the floor's rms target keep their ratio.
    assert!(tr.iter().any(|x| (x.1 + 6.0).abs() < 0.05));
    assert!(tr.iter().any(|x| x.1.abs() < 0.05));
    let ratio = |x: &(f64, f64, f64, f64, bool)| x.2 / x.3;
    let r0 = ratio(first);
    assert!(tr[..up]
        .iter()
        .filter(|x| x.2 > 0.0)
        .all(|x| (ratio(x) / r0 - 1.0).abs() < 1e-6));
    // The full-level time runs out and the run completes.
    assert_eq!(s.state(), MixedState::Stopped);
    assert_eq!(
        s.report_data().outcome,
        Some(hertzforge_mixed::MixedOutcome::Completed)
    );
}

#[test]
fn a_hold_stops_the_rungs_clock_too() {
    let mut s = engaged(ladder_cfg(), DELAY);
    // Into the parts' run at -9 dB.
    while s.live_status().phase != MixedPhase::Running {
        assert!(run_for(&mut s, 0.25));
    }
    run_for(&mut s, 1.0);
    s.hold().unwrap();
    let held = trace(&mut s, 10.0);
    assert!(
        held.iter().all(|x| (x.1 + 9.0).abs() < 0.05),
        "the rung did not wait"
    );
    s.resume().unwrap();
    let after = trace(&mut s, 8.0);
    assert!(after.iter().any(|x| (x.1 + 6.0).abs() < 0.05));
}
