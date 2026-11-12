//! The level ladder, with the level-step overshoot guard.
//!
//! - A level is left only on a judged pass (every control passing). A level
//!   whose allowance of pulses is spent without a pass ends the run.
//! - Every level but full level needs one passing pulse; full level needs
//!   `full_level_pulses`.
//! - **The step-overshoot guard.** The worst signed error (measured over
//!   target, dB, positive an over-test) of the first pulse at each level is
//!   kept. Before stepping up, the first pulse's over-test at the next level is
//!   predicted from those first pulses: the last one's value when there is one,
//!   the last two's straight line in level, extrapolated, when there are two.
//!   While the prediction is over the upper tolerance, the step is halved, down
//!   to a floor of [`MIN_STEP_DB`]; the run's record says so.

use serde::{Deserialize, Serialize};

/// The smallest step the overshoot guard halves to, dB.
pub const MIN_STEP_DB: f64 = 0.75;

/// What one judged pulse did to the ladder.
#[derive(Debug, Clone, PartialEq)]
pub enum Tick {
    /// The level stays.
    Held,
    /// The ladder moved up to `to`, dB; `note` says when the step was halved.
    Advanced { to: f64, note: Option<StepNote> },
    /// Full level passed its pulses.
    Completed,
    /// The level's allowance was spent without a pass.
    NotConverged { pulses: u32 },
}

/// A step the overshoot guard halved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepNote {
    /// The level the step left, dB.
    pub from_db: f64,
    /// The next level of the table, and the level actually taken, dB.
    pub planned_db: f64,
    pub taken_db: f64,
    /// The over-test predicted at the planned level and at the level taken, dB.
    pub predicted_planned_db: f64,
    pub predicted_taken_db: f64,
    /// The upper tolerance the prediction is held to, dB.
    pub tolerance_db: f64,
}

/// The ladder's state.
#[derive(Debug, Clone, PartialEq)]
pub struct Ladder {
    table: Vec<f64>,
    level: f64,
    full_level_pulses: u32,
    max_without_pass: u32,
    tolerance_db: f64,
    passes: u32,
    without_pass: u32,
    first_pending: bool,
    firsts: Vec<(f64, f64)>,
}

impl Ladder {
    /// A ladder on `table` (ascending, ending at 0 dB), starting at its first
    /// level, holding first-pulse over-tests to `tolerance_db`.
    pub fn new(
        table: Vec<f64>,
        full_level_pulses: u32,
        max_without_pass: u32,
        tolerance_db: f64,
    ) -> Self {
        let level = table[0];
        Ladder {
            table,
            level,
            full_level_pulses: full_level_pulses.max(1),
            max_without_pass: max_without_pass.max(1),
            tolerance_db,
            passes: 0,
            without_pass: 0,
            first_pending: true,
            firsts: Vec::new(),
        }
    }

    /// The level now, dB.
    pub fn level_db(&self) -> f64 {
        self.level
    }

    /// The first pulses' over-tests so far, `(level_db, over_test_db)`.
    pub fn first_pulses(&self) -> &[(f64, f64)] {
        &self.firsts
    }

    fn full(&self) -> bool {
        self.level >= -1e-9
    }

    /// The first pulse's over-test predicted at `level_db`.
    pub fn predict(&self, level_db: f64) -> Option<f64> {
        match self.firsts.as_slice() {
            [] => None,
            [(_, e1)] => Some(*e1),
            [.., (l0, e0), (l1, e1)] => {
                let slope = if (l1 - l0).abs() > 1e-12 {
                    (e1 - e0) / (l1 - l0)
                } else {
                    0.0
                };
                Some(e1 + slope * (level_db - l1))
            }
        }
    }

    /// Tick after a pulse at the current level was judged: `passed` (every
    /// control) and its worst signed error `over_test_db`.
    pub fn on_judged(&mut self, passed: bool, over_test_db: f64) -> Tick {
        if self.first_pending {
            self.firsts.push((self.level, over_test_db));
            self.first_pending = false;
        }
        if !passed {
            self.without_pass += 1;
            if self.without_pass >= self.max_without_pass {
                return Tick::NotConverged {
                    pulses: self.without_pass,
                };
            }
            return Tick::Held;
        }
        self.without_pass = 0;
        self.passes += 1;
        if self.full() {
            return if self.passes >= self.full_level_pulses {
                Tick::Completed
            } else {
                Tick::Held
            };
        }
        let planned = self
            .table
            .iter()
            .copied()
            .find(|l| *l > self.level + 1e-9)
            .unwrap_or(0.0);
        let mut step = planned - self.level;
        let mut note = None;
        if let Some(p_planned) = self.predict(planned) {
            if p_planned > self.tolerance_db {
                while step / 2.0 >= MIN_STEP_DB - 1e-9
                    && self.predict(self.level + step).unwrap_or(0.0) > self.tolerance_db
                {
                    step /= 2.0;
                }
                note = Some(StepNote {
                    from_db: self.level,
                    planned_db: planned,
                    taken_db: self.level + step,
                    predicted_planned_db: p_planned,
                    predicted_taken_db: self.predict(self.level + step).unwrap_or(p_planned),
                    tolerance_db: self.tolerance_db,
                });
            }
        }
        self.level += step;
        if self.level.abs() < 1e-9 {
            self.level = 0.0;
        }
        self.passes = 0;
        self.first_pending = true;
        Tick::Advanced {
            to: self.level,
            note,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: [f64; 6] = [-12.0, -9.0, -6.0, -3.0, -1.5, 0.0];

    #[test]
    fn mimo_shock_engine_the_ladder_never_steps_up_on_a_failing_level() {
        let mut l = Ladder::new(TABLE.to_vec(), 3, 3, 3.0);
        assert_eq!(l.on_judged(false, 0.0), Tick::Held);
        assert_eq!(l.on_judged(false, 0.0), Tick::Held);
        assert_eq!(l.level_db(), -12.0);
        assert_eq!(l.on_judged(false, 0.0), Tick::NotConverged { pulses: 3 });
        assert_eq!(l.level_db(), -12.0);
    }

    #[test]
    fn mimo_shock_engine_the_ladder_takes_each_level_on_a_pass_and_completes_at_full() {
        let mut l = Ladder::new(TABLE.to_vec(), 3, 3, 3.0);
        for want in [-9.0, -6.0, -3.0, -1.5, 0.0] {
            assert_eq!(
                l.on_judged(true, -0.5),
                Tick::Advanced {
                    to: want,
                    note: None
                }
            );
        }
        assert_eq!(l.on_judged(true, -0.5), Tick::Held);
        assert_eq!(l.on_judged(false, -0.5), Tick::Held);
        assert_eq!(l.on_judged(true, -0.5), Tick::Held);
        assert_eq!(l.on_judged(true, -0.5), Tick::Completed);
    }

    #[test]
    fn mimo_shock_engine_the_step_is_halved_when_the_trend_predicts_an_over_test() {
        let mut l = Ladder::new(TABLE.to_vec(), 3, 3, 3.0);
        // First pulses at -12 and -9 over-test 0.5 and 2.0 dB: 0.5 dB a dB of
        // level, so -6 would be 3.5 dB over, -7.5 2.75 dB.
        l.on_judged(true, 0.5);
        match l.on_judged(true, 2.0) {
            Tick::Advanced { to, note: Some(n) } => {
                assert_eq!(to, -7.5);
                assert!((n.predicted_planned_db - 3.5).abs() < 1e-12);
                assert!((n.predicted_taken_db - 2.75).abs() < 1e-12);
            }
            t => panic!("{t:?}"),
        }
        // Steep: halving stops at the floor.
        let mut l = Ladder::new(TABLE.to_vec(), 3, 3, 3.0);
        l.on_judged(true, 0.0);
        match l.on_judged(true, 2.9) {
            Tick::Advanced { to, note: Some(n) } => {
                assert_eq!(to, -9.0 + MIN_STEP_DB);
                assert!(n.predicted_taken_db > 3.0);
            }
            t => panic!("{t:?}"),
        }
        // One first pulse: its own value is the prediction.
        let mut l = Ladder::new(TABLE.to_vec(), 3, 3, 3.0);
        match l.on_judged(true, 3.5) {
            Tick::Advanced { to, note: Some(_) } => assert_eq!(to, -12.0 + MIN_STEP_DB),
            t => panic!("{t:?}"),
        }
    }
}
