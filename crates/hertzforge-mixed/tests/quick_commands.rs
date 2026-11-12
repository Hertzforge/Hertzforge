//! Commands asked for faster than the run takes them up.
//!
//! A verb (a part switch, a level change) is posted to the run and takes
//! effect at its next batch; the status reports what is in force. Between the
//! two, a second command has to be judged on what the first one asked for, not
//! on what the status still shows. These tests post commands with no chunk
//! between them. Each names the change to the source that makes it fail.

mod common;
use common::*;
use hertzforge_mixed::{MixedPhase, PartKind};
use hertzforge_random::service::config::LevelSchedule;
use hertzforge_random::RunMode;

fn band_row(s: &hertzforge_mixed::MixedService) -> hertzforge_mixed::PartRow {
    s.live_status()
        .parts
        .into_iter()
        .find(|r| r.kind == PartKind::Narrowband)
        .expect("the band's row")
}

/// Mutation: in `MixedService::part_switch`, leave the queued switches out and
/// answer with the switch in force. The second assertion block then reads
/// `Some(true)` where the part has just been switched off.
#[test]
fn a_switch_asked_for_and_not_yet_taken_up_is_the_switch_to_decide_on() {
    let cfg = config(vec![floor(0.0005), band("b1", fixed(300.0), 0.002, 20.0)]);
    let mut s = engaged(cfg, DELAY);
    assert!(run_for(&mut s, 30.0), "{:?}", s.end_reason());
    assert_eq!(s.part_switch("b1"), Some(true));
    assert_eq!(s.part_switch("floor"), Some(true));
    assert_eq!(s.part_switch("no such part"), None);

    // Off, with no chunk run: the status still shows the switch in force, the
    // switch to decide on is the one asked for.
    s.set_part_enabled("b1", false).unwrap();
    assert!(
        band_row(&s).enabled,
        "the status is as new as the last chunk"
    );
    assert_eq!(s.part_switch("b1"), Some(false));

    // On again, still with no chunk between: the last one asked for stands.
    s.set_part_enabled("b1", true).unwrap();
    assert_eq!(s.part_switch("b1"), Some(true));
    assert!(run_for(&mut s, 5.0), "{:?}", s.end_reason());
    let r = band_row(&s);
    assert!(r.enabled && r.on, "enabled {} on {}", r.enabled, r.on);
    assert!((r.scale - 1.0).abs() < 1e-9, "scale {}", r.scale);
    assert_eq!(s.part_switch("b1"), Some(true));

    // And the other way round: on (already on), then off, leaves it off.
    s.set_part_enabled("b1", true).unwrap();
    s.set_part_enabled("b1", false).unwrap();
    assert_eq!(s.part_switch("b1"), Some(false));
    assert!(run_for(&mut s, 5.0), "{:?}", s.end_reason());
    let r = band_row(&s);
    assert!(!r.enabled && !r.on, "enabled {} on {}", r.enabled, r.on);
    assert!(r.scale.abs() < 1e-9, "scale {}", r.scale);
}

/// Mutation: in `MixedService::level_db_now`, leave the queued level out and
/// step from the level in force. The second step then answers -4 dB and the
/// run settles 2 dB short.
#[test]
fn two_level_steps_before_the_next_batch_add_up() {
    let mut cfg = config(vec![floor(0.0005), band("b1", fixed(300.0), 0.002, 20.0)]);
    cfg.schedule = LevelSchedule {
        start_db: -6.0,
        table_db: vec![-6.0, 0.0],
        ..LevelSchedule::auto_default()
    };
    let mut s = engaged(cfg, DELAY);
    s.set_run_mode(RunMode::Manual).unwrap();
    assert!(run_for(&mut s, 30.0), "{:?}", s.end_reason());
    let st = s.live_status();
    assert_eq!(st.phase, MixedPhase::Running);
    assert!((st.level_db + 6.0).abs() < 1e-6, "at {} dB", st.level_db);

    let first = s.step_level(2.0).unwrap();
    assert!((first + 4.0).abs() < 1e-9, "the first step took {first} dB");
    let second = s.step_level(2.0).unwrap();
    assert!(
        (second + 2.0).abs() < 1e-9,
        "the second step starts where the first one is going, not at {second} dB"
    );
    assert!(run_for(&mut s, 10.0), "{:?}", s.end_reason());
    let st = s.live_status();
    assert!(
        (st.level_db + 2.0).abs() < 1e-6,
        "settled at {} dB",
        st.level_db
    );

    // A step past the top of the table is held at the top, and says so.
    assert!(s.step_level(5.0).unwrap().abs() < 1e-9);
    assert!(s.step_level(5.0).unwrap().abs() < 1e-9);
}
