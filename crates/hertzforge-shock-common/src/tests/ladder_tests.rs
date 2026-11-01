//! Shock level-ladder state-machine tests, mirroring hertzforge-random's
//! ladder tests with shock (N-pulses-per-level) semantics.

use crate::ladder::{
    LadderError, ShockLadderState, ShockLevelSchedule, ShockRunMode, DEFAULT_LEVEL_TABLE_DB,
    DEFAULT_START_LEVEL_DB,
};

#[test]
fn auto_default_has_h1_ladder_shape() {
    let s = ShockLevelSchedule::auto_default();
    assert_eq!(s.mode, ShockRunMode::Auto);
    assert_eq!(s.start_db, DEFAULT_START_LEVEL_DB);
    assert_eq!(s.table_db, DEFAULT_LEVEL_TABLE_DB.to_vec());
    assert_eq!(s.full_db(), 0.0);
    s.validate().unwrap();
}

#[test]
fn auto_walks_the_table_one_pulse_per_level_to_completion() {
    let s = ShockLevelSchedule::auto_default(); // 1 pulse per level, 5 levels
    let mut st = ShockLadderState::new(s).unwrap();
    assert_eq!(st.current_level_db(), -12.0);
    assert_eq!(st.level_index(), 0);

    let expected = [-12.0, -9.0, -6.0, -3.0, 0.0];
    // Fire one pulse at each level; after each the ladder advances.
    for (i, exp) in expected.iter().enumerate() {
        assert_eq!(st.current_level_db(), *exp, "level {i}");
        st.on_pulse_fired();
    }
    // After the last level's pulse, the run is complete and holds at full.
    assert!(st.completed());
    assert_eq!(st.current_level_db(), 0.0);
}

#[test]
fn auto_holds_each_level_for_n_pulses() {
    let s = ShockLevelSchedule {
        mode: ShockRunMode::Auto,
        start_db: -12.0,
        table_db: vec![-12.0, -6.0, 0.0],
        pulses_per_level: vec![3], // 3 pulses per level
    };
    let mut st = ShockLadderState::new(s).unwrap();
    // Fire 3 at -12 -> still at -12 until the 3rd advances it.
    for _ in 0..2 {
        st.on_pulse_fired();
        assert_eq!(st.current_level_db(), -12.0);
    }
    st.on_pulse_fired(); // 3rd pulse -> advance
    assert_eq!(st.current_level_db(), -6.0);
    assert_eq!(st.pulses_at_level(), 0);
}

#[test]
fn per_level_pulse_counts_can_vary() {
    let s = ShockLevelSchedule {
        mode: ShockRunMode::Auto,
        start_db: -6.0,
        table_db: vec![-6.0, 0.0],
        pulses_per_level: vec![2, 1],
    };
    let mut st = ShockLadderState::new(s).unwrap();
    st.on_pulse_fired();
    assert_eq!(st.current_level_db(), -6.0); // still level 0 after 1/2
    st.on_pulse_fired();
    assert_eq!(st.current_level_db(), 0.0); // advanced after 2/2
    st.on_pulse_fired();
    assert!(st.completed()); // 1/1 at full completes
}

#[test]
fn manual_steps_plus_minus_one_db_and_does_not_auto_advance() {
    let s = ShockLevelSchedule::manual_default();
    let mut st = ShockLadderState::new(s).unwrap();
    assert_eq!(st.current_level_db(), -12.0);
    // Firing pulses in MANUAL does not advance the level.
    st.on_pulse_fired();
    st.on_pulse_fired();
    assert_eq!(st.current_level_db(), -12.0);
    assert!(!st.completed());
    // +/- 1 dB steps (start -12; three +1 steps -> -9; one -1 step -> -10).
    st.step_up();
    assert_eq!(st.current_level_db(), -11.0);
    st.step_up();
    st.step_up();
    assert_eq!(st.current_level_db(), -9.0);
    st.step_down();
    assert_eq!(st.current_level_db(), -10.0);
}

#[test]
fn jump_to_level_resets_offset_and_counter() {
    let s = ShockLevelSchedule::manual_default();
    let mut st = ShockLadderState::new(s).unwrap();
    st.step_up(); // -11 dB (manual offset +1)
    st.on_pulse_fired();
    st.jump_to_level(3).unwrap(); // -3 dB table level
    assert_eq!(st.current_level_db(), -3.0, "jump clears the manual offset");
    assert_eq!(st.level_index(), 3);
    assert_eq!(st.pulses_at_level(), 0);
    // Out-of-range jump is rejected.
    assert!(matches!(
        st.jump_to_level(99),
        Err(LadderError::BadLevelIndex { .. })
    ));
}

#[test]
fn current_scale_is_ten_to_the_db_over_twenty() {
    let s = ShockLevelSchedule::manual_default();
    let mut st = ShockLadderState::new(s).unwrap();
    // -12 dB -> 10^(-12/20) ~ 0.2512
    assert!((st.current_scale() - 10.0_f64.powf(-12.0 / 20.0)).abs() < 1e-12);
    st.jump_to_level(4).unwrap(); // 0 dB -> scale 1.0
    assert!((st.current_scale() - 1.0).abs() < 1e-12);
}

#[test]
fn start_level_off_the_table_is_carried_as_a_manual_offset() {
    // A start level that is not a table entry snaps to the nearest index and carries
    // the remainder as an offset so current_level_db == start_db.
    let s = ShockLevelSchedule {
        mode: ShockRunMode::Manual,
        start_db: -10.0, // between -12 and -9, nearest is -9
        table_db: vec![-12.0, -9.0, -6.0, 0.0],
        pulses_per_level: vec![1],
    };
    let st = ShockLadderState::new(s).unwrap();
    assert_eq!(st.current_level_db(), -10.0);
    assert_eq!(st.level_index(), 1); // snapped to -9
}

#[test]
fn schedule_validation_rejects_bad_tables() {
    // Empty table.
    let bad = ShockLevelSchedule {
        mode: ShockRunMode::Auto,
        start_db: 0.0,
        table_db: vec![],
        pulses_per_level: vec![1],
    };
    assert!(matches!(bad.validate(), Err(LadderError::BadSchedule(_))));

    // Decreasing table.
    let bad = ShockLevelSchedule {
        mode: ShockRunMode::Auto,
        start_db: 0.0,
        table_db: vec![0.0, -3.0],
        pulses_per_level: vec![1],
    };
    assert!(matches!(bad.validate(), Err(LadderError::BadSchedule(_))));

    // pulses_per_level length mismatch (not 1, not table len).
    let bad = ShockLevelSchedule {
        mode: ShockRunMode::Auto,
        start_db: -6.0,
        table_db: vec![-6.0, -3.0, 0.0],
        pulses_per_level: vec![1, 2],
    };
    assert!(matches!(bad.validate(), Err(LadderError::BadSchedule(_))));

    // AUTO with a zero pulse count.
    let bad = ShockLevelSchedule {
        mode: ShockRunMode::Auto,
        start_db: 0.0,
        table_db: vec![0.0],
        pulses_per_level: vec![0],
    };
    assert!(matches!(bad.validate(), Err(LadderError::BadSchedule(_))));
}

#[test]
fn ladder_round_trips_through_json() {
    let s = ShockLevelSchedule::auto_default();
    let mut st = ShockLadderState::new(s).unwrap();
    st.on_pulse_fired();
    let js = serde_json::to_string(&st).unwrap();
    let back: ShockLadderState = serde_json::from_str(&js).unwrap();
    assert_eq!(st, back);
}
