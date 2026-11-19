//! The floor's status states the degrees of freedom its estimate holds: the
//! typical clean line's, not the line a swept tone has just left. On the
//! lagged resonant plant of `common`, a tone sweeping through the floor's
//! band frees a line or two every frame, each holding one frame (2 degrees
//! of freedom), while nearly every clean line holds the full depth.

mod common;
use common::*;
use hertzforge_mixed::{
    parts::{Direction, PartSchedule, PartSweepTarget, SweepLaw},
    MixedPhase,
};

#[test]
fn the_floor_reports_the_degrees_of_freedom_its_clean_lines_hold_with_a_tone_sweeping() {
    let sweep = PartSchedule::Sweep {
        lo_hz: 150.0,
        hi_hz: 450.0,
        start_hz: 150.0,
        direction: Direction::Up,
        law: SweepLaw::LinearHzPerS,
        rate: 4.0,
        target: PartSweepTarget::Sweeps { n: 4 },
    };
    let mut c = config(vec![floor(0.01), tone("s", sweep, 1.0)]);
    c.soft_start_db = 0.0;
    c.floor_settle_s = 3.0;
    let mut s = engaged(c, DELAY);
    while s.live_status().phase != MixedPhase::Running {
        assert!(run_for(&mut s, 0.25), "{:?}", s.end_reason());
    }
    // Eight loops (a second each) with the tone sweeping.
    let mut read = Vec::new();
    for _ in 0..32 {
        assert!(run_for(&mut s, 0.25), "{:?}", s.end_reason());
        let f = s.live_status().floor;
        read.push((f.dof, f.lines_in_band - f.lines_out, f.lines_in_band));
    }
    let (dof, clean, lines) = *read.last().unwrap();
    eprintln!(
        "after eight loops with the tone sweeping: {dof:.1} dof, {clean} of {lines} lines clean"
    );
    // The depth is control_dof (60) worth of frames: the estimate holds it.
    assert!(dof >= 55.0, "{read:?}");
}
