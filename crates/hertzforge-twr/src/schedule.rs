//! Level scheduling (the convergence-gated dB ladder) + the run schedule.
//!
//! # The ladder (MIL-STD-810 525.2's ramp; the house ladder shape)
//!
//! Same `[-12, -9, -6, -3, 0]` dB table as Random and Shock,
//! with the dwell/pulse-count advance replaced by a CONVERGENCE GATE: a level
//! advances when `rms_error_pct <= gate_rms_error_pct`, and fails (policy: abort
//! or hold) when `max_iterations_per_level` passes without gating. Level steps
//! carry the converged drive forward scaled by the step ratio.
//!
//! Pure state machines - no I/O, no timers; the service ticks them.

use serde::{Deserialize, Serialize};

use crate::TwrError;

/// Default AUTO table: 3 dB steps from -12 dB to full.
pub const DEFAULT_LEVEL_TABLE_DB: [f64; 5] = [-12.0, -9.0, -6.0, -3.0, 0.0];
/// Manual trim step, dB.
pub const MANUAL_STEP_DB: f64 = 1.0;

/// How the run advances through levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TwrRunMode {
    /// Walk the table, gating each level on convergence.
    Auto,
    /// The operator iterates and steps levels by hand.
    Manual,
}

/// What to do when a level exhausts its iterations without gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateFailPolicy {
    /// Abort the run (default - the safety-conservative choice).
    Abort,
    /// Hold at the level, stop iterating, wait for the operator.
    Hold,
}

/// The level schedule (immutable config; the run position lives in
/// [`TwrLadderState`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TwrLevelSchedule {
    pub mode: TwrRunMode,
    /// Level table, dB relative to full (non-empty, non-decreasing).
    pub table_db: Vec<f64>,
    /// Convergence gate: a level advances at `rms_error_pct <=` this. Default 10.
    pub gate_rms_error_pct: f64,
    /// Max correction passes per level before the gate-fail policy applies.
    pub max_iterations_per_level: u32,
    pub on_gate_fail: GateFailPolicy,
}

impl Default for TwrLevelSchedule {
    fn default() -> Self {
        TwrLevelSchedule {
            mode: TwrRunMode::Auto,
            table_db: DEFAULT_LEVEL_TABLE_DB.to_vec(),
            gate_rms_error_pct: 10.0,
            max_iterations_per_level: 8,
            on_gate_fail: GateFailPolicy::Abort,
        }
    }
}

impl TwrLevelSchedule {
    pub fn validate(&self) -> Result<(), TwrError> {
        if self.table_db.is_empty() {
            return Err(TwrError::Config("level table is empty".into()));
        }
        for w in self.table_db.windows(2) {
            if w[1] < w[0] {
                return Err(TwrError::Config(format!(
                    "level table is not non-decreasing ({} then {})",
                    w[0], w[1]
                )));
            }
        }
        if self.gate_rms_error_pct.is_nan() || self.gate_rms_error_pct <= 0.0 {
            return Err(TwrError::Config("gate_rms_error_pct must be > 0".into()));
        }
        if self.max_iterations_per_level == 0 {
            return Err(TwrError::Config(
                "max_iterations_per_level must be >= 1".into(),
            ));
        }
        Ok(())
    }

    /// The full level (last table entry).
    pub fn full_db(&self) -> f64 {
        self.table_db.last().copied().unwrap_or(0.0)
    }
}

/// The mutable ladder position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TwrLadderState {
    schedule: TwrLevelSchedule,
    level_index: usize,
    iterations_at_level: u32,
    manual_offset_db: f64,
    /// True once the CURRENT level has gated.
    converged_at_level: bool,
    /// True once the TOP level has gated (the run may move to playback).
    completed: bool,
}

impl TwrLadderState {
    pub fn new(schedule: TwrLevelSchedule) -> Result<Self, TwrError> {
        schedule.validate()?;
        Ok(TwrLadderState {
            schedule,
            level_index: 0,
            iterations_at_level: 0,
            manual_offset_db: 0.0,
            converged_at_level: false,
            completed: false,
        })
    }

    pub fn schedule(&self) -> &TwrLevelSchedule {
        &self.schedule
    }

    pub fn current_level_db(&self) -> f64 {
        self.schedule.table_db[self.level_index] + self.manual_offset_db
    }

    /// Linear scale `10^(level/20)` the target (and carried drive) multiply by.
    pub fn current_scale(&self) -> f64 {
        10f64.powf(self.current_level_db() / 20.0)
    }

    pub fn level_index(&self) -> usize {
        self.level_index
    }

    pub fn iterations_at_level(&self) -> u32 {
        self.iterations_at_level
    }

    pub fn converged_at_level(&self) -> bool {
        self.converged_at_level
    }

    pub fn completed(&self) -> bool {
        self.completed
    }

    /// Record one correction pass at the current level; returns the pass number
    /// (1-based).
    pub fn on_iteration(&mut self) -> u32 {
        self.iterations_at_level = self.iterations_at_level.saturating_add(1);
        self.iterations_at_level
    }

    /// True when the level has spent its iteration budget without gating.
    pub fn budget_exhausted(&self) -> bool {
        !self.converged_at_level
            && self.iterations_at_level >= self.schedule.max_iterations_per_level
    }

    /// The gate check: call with each pass's error. Marks the level converged
    /// when the gate passes.
    pub fn check_gate(&mut self, rms_error_pct: f64) -> bool {
        if rms_error_pct <= self.schedule.gate_rms_error_pct {
            self.converged_at_level = true;
        }
        self.converged_at_level
    }

    /// Advance to the next table level (after a gate pass). Returns the dB STEP
    /// applied (new - old, for drive carry-scaling), or `None` at the top (sets
    /// [`Self::completed`]).
    pub fn advance_level(&mut self) -> Option<f64> {
        if !self.converged_at_level {
            return None;
        }
        if self.level_index + 1 < self.schedule.table_db.len() {
            let old = self.current_level_db();
            self.level_index += 1;
            self.iterations_at_level = 0;
            self.converged_at_level = false;
            Some(self.current_level_db() - old)
        } else {
            self.completed = true;
            None
        }
    }

    /// Manual +1 dB trim (allowed in any mode; moves the offset only).
    pub fn step_up(&mut self) {
        self.manual_offset_db += MANUAL_STEP_DB;
        self.converged_at_level = false;
        self.iterations_at_level = 0;
    }

    /// Manual -1 dB trim.
    pub fn step_down(&mut self) {
        self.manual_offset_db -= MANUAL_STEP_DB;
        self.converged_at_level = false;
        self.iterations_at_level = 0;
    }

    /// The dB step a jump to table level `i` WOULD apply, without taking it.
    ///
    /// The caller needs this to run the drive ceiling before anything moves: a
    /// refusal must not leave the ladder at a level the drive never followed it
    /// to.
    pub fn jump_step_to(&self, i: usize) -> Result<f64, TwrError> {
        let target = self.schedule.table_db.get(i).copied().ok_or_else(|| {
            TwrError::Config(format!(
                "jump-to-level {i} out of range (table has {})",
                self.schedule.table_db.len()
            ))
        })?;
        // A jump clears the manual offset, so the step is measured against the
        // bare table entry.
        Ok(target - self.current_level_db())
    }

    /// Jump to table level `i`, clearing the manual offset and counters.
    pub fn jump_to_level(&mut self, i: usize) -> Result<f64, TwrError> {
        if i >= self.schedule.table_db.len() {
            return Err(TwrError::Config(format!(
                "jump-to-level {i} out of range (table has {})",
                self.schedule.table_db.len()
            )));
        }
        let old = self.current_level_db();
        self.level_index = i;
        self.manual_offset_db = 0.0;
        self.iterations_at_level = 0;
        self.converged_at_level = false;
        self.completed = false;
        Ok(self.current_level_db() - old)
    }
}

/// The playback schedule once the full level has converged.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RunSchedule {
    /// Play the record once.
    Once,
    /// Loop the record N times.
    LoopN(u32),
    /// Play repeatedly until `seconds` of SECTION time have elapsed
    /// (`ceil(seconds / section_duration)` plays).
    DurationS(f64),
}

impl RunSchedule {
    /// How many plays this schedule needs for a section of `section_s` seconds.
    pub fn plays_target(&self, section_s: f64) -> u32 {
        match *self {
            RunSchedule::Once => 1,
            RunSchedule::LoopN(n) => n.max(1),
            RunSchedule::DurationS(s) => {
                if section_s <= 0.0 {
                    1
                } else {
                    (s / section_s).ceil().max(1.0) as u32
                }
            }
        }
    }

    pub fn validate(&self) -> Result<(), TwrError> {
        match *self {
            RunSchedule::LoopN(0) => Err(TwrError::Config("LoopN needs N >= 1".into())),
            RunSchedule::DurationS(s) if s.is_nan() || s <= 0.0 => {
                Err(TwrError::Config("DurationS needs seconds > 0".into()))
            }
            _ => Ok(()),
        }
    }
}

/// A from-to slice of the reference to replicate (sample indices into the
/// PREPARED reference; `len == 0` is invalid).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub start: usize,
    pub len: usize,
}

impl Section {
    /// The whole reference.
    pub fn all(n: usize) -> Section {
        Section { start: 0, len: n }
    }

    pub fn validate(&self, reference_len: usize) -> Result<(), TwrError> {
        if self.len == 0 {
            return Err(TwrError::Config("section length is zero".into()));
        }
        if self.start + self.len > reference_len {
            return Err(TwrError::Config(format!(
                "section [{}..{}) exceeds the reference length {}",
                self.start,
                self.start + self.len,
                reference_len
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_walks_on_gate_passes_and_carries_the_step() {
        let mut l = TwrLadderState::new(TwrLevelSchedule::default()).unwrap();
        assert_eq!(l.current_level_db(), -12.0);
        l.on_iteration();
        assert!(!l.check_gate(50.0));
        l.on_iteration();
        assert!(l.check_gate(9.0));
        let step = l.advance_level().unwrap();
        assert_eq!(step, 3.0);
        assert_eq!(l.current_level_db(), -9.0);
        assert_eq!(l.iterations_at_level(), 0);
        // Walk to the top.
        for _ in 0..3 {
            l.check_gate(5.0);
            l.advance_level();
        }
        assert_eq!(l.current_level_db(), 0.0);
        l.check_gate(5.0);
        assert!(l.advance_level().is_none());
        assert!(l.completed());
    }

    #[test]
    fn budget_exhaustion_flags() {
        let sched = TwrLevelSchedule {
            max_iterations_per_level: 2,
            ..TwrLevelSchedule::default()
        };
        let mut l = TwrLadderState::new(sched).unwrap();
        l.on_iteration();
        assert!(!l.budget_exhausted());
        l.on_iteration();
        assert!(!l.check_gate(50.0));
        assert!(l.budget_exhausted());
        // Gating clears it.
        assert!(l.check_gate(1.0));
        assert!(!l.budget_exhausted());
    }

    #[test]
    fn manual_trim_and_jump() {
        let mut l = TwrLadderState::new(TwrLevelSchedule::default()).unwrap();
        l.step_up();
        assert_eq!(l.current_level_db(), -11.0);
        l.step_down();
        l.step_down();
        assert_eq!(l.current_level_db(), -13.0);
        let step = l.jump_to_level(4).unwrap();
        assert_eq!(l.current_level_db(), 0.0);
        assert!((step - 13.0).abs() < 1e-12);
        assert!(l.jump_to_level(9).is_err());
    }

    #[test]
    fn schedule_validation() {
        let mut s = TwrLevelSchedule {
            table_db: vec![-6.0, -9.0],
            ..TwrLevelSchedule::default()
        };
        assert!(s.validate().is_err());
        s.table_db = vec![];
        assert!(s.validate().is_err());
        assert!(TwrLevelSchedule::default().validate().is_ok());
    }

    #[test]
    fn run_schedule_play_targets() {
        assert_eq!(RunSchedule::Once.plays_target(0.75), 1);
        assert_eq!(RunSchedule::LoopN(5).plays_target(0.75), 5);
        assert_eq!(RunSchedule::DurationS(2.0).plays_target(0.75), 3);
        assert!(RunSchedule::LoopN(0).validate().is_err());
        assert!(RunSchedule::DurationS(0.0).validate().is_err());
    }

    #[test]
    fn section_bounds() {
        assert!(Section { start: 0, len: 0 }.validate(10).is_err());
        assert!(Section { start: 8, len: 3 }.validate(10).is_err());
        assert!(Section { start: 2, len: 8 }.validate(10).is_ok());
        assert_eq!(Section::all(7), Section { start: 0, len: 7 });
    }
}
