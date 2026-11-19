//! The shock level-ladder state machine, mirroring
//! hertzforge-random's ladder shape with SHOCK semantics.
//!
//! # Same shape as Random's ladder, different clock
//!
//! Random's [`hertzforge_random::service::config::LevelSchedule`] walks a dB table with a
//! per-level DWELL time (random vibration is continuous, so a level is held for
//! seconds). Shock is a sequence of discrete pulses, so the shock ladder holds each
//! level for N PULSES instead of a dwell time - otherwise the shape is identical:
//! the default table is `[-12, -9, -6, -3, 0]` dB, the operator can step `+/- 1 dB`
//! and jump to any table level, and AUTO walks the table while MANUAL steps from a
//! start level.
//!
//! This module is a PURE state machine (no I/O, no timers): the service ticks it once
//! per fired pulse via [`ShockLadderState::on_pulse_fired`] and reads the current
//! commanded level via [`ShockLadderState::current_level_db`]. Every transition is a
//! function of the schedule + the pulse count, so it is exhaustively testable.

use serde::{Deserialize, Serialize};

/// The default START level once the run engages (shock convention):
/// -12 dB. Same start as Random's ladder.
pub const DEFAULT_START_LEVEL_DB: f64 = -12.0;

/// The default AUTO level table: 3 dB steps from -12 dB to full. Same
/// table as Random's ladder.
pub const DEFAULT_LEVEL_TABLE_DB: [f64; 5] = [-12.0, -9.0, -6.0, -3.0, 0.0];

/// The default number of pulses fired per level in AUTO. Shock holds a
/// level for a pulse COUNT, not a dwell time.
pub const DEFAULT_PULSES_PER_LEVEL: u32 = 1;

/// The manual step size in dB: the operator nudges `+/- 1 dB`.
pub const MANUAL_STEP_DB: f64 = 1.0;

/// The default number of pulses at one level without a passing pulse (AUTO
/// with [`LadderAdvance::OnPass`]), or failing pulses in a row at one MANUAL
/// level, before a closed-loop run ends as not converged.
pub const DEFAULT_MAX_PULSES_PER_LEVEL: u32 = 5;

/// When the AUTO ladder moves to the next level.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LadderAdvance {
    /// After the level's pulse count, whatever the pulses' verdicts (the
    /// open-loop ladder, and SRS).
    #[default]
    Count,
    /// Only on a passing pulse: a level's pulse count counts passing pulses,
    /// and [`ShockLevelSchedule::max_pulses_per_level`] pulses at one level
    /// without a pass end the run. For a closed loop, where early pulses are
    /// how the law learns.
    OnPass,
}

impl LadderAdvance {
    /// True for [`LadderAdvance::Count`], the default.
    pub fn is_count(&self) -> bool {
        *self == LadderAdvance::Count
    }
}

fn default_max_pulses_per_level() -> u32 {
    DEFAULT_MAX_PULSES_PER_LEVEL
}

fn is_default_max_pulses_per_level(n: &u32) -> bool {
    *n == DEFAULT_MAX_PULSES_PER_LEVEL
}

/// How the run advances through levels, mirroring Random's `RunMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShockRunMode {
    /// AUTO: fire `pulses_per_level` pulses at each table level, advancing up the
    /// table, then hold at full.
    Auto,
    /// MANUAL: the operator steps levels with `+/- 1 dB` / jump-to-table-level; no
    /// automatic progression on a fired pulse.
    Manual,
}

/// The shock level schedule. All levels are dB relative to full (0 dB =
/// full reference pulse). AUTO walks `table_db` firing `pulses_per_level` pulses each;
/// MANUAL starts at `start_db` and the operator steps from there.
///
/// This is the immutable CONFIG; the mutable run position lives in
/// [`ShockLadderState`]. Mirrors Random's `LevelSchedule` field-for-field with the
/// dwell replaced by a pulse count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShockLevelSchedule {
    pub mode: ShockRunMode,
    /// The engage/start level (default -12 dB).
    pub start_db: f64,
    /// The AUTO level table in dB (default `[-12, -9, -6, -3, 0]`). Must be non-empty
    /// and non-decreasing for AUTO.
    pub table_db: Vec<f64>,
    /// Pulses fired per table level in AUTO (one entry per table level, or a single
    /// value applied to every level if length 1). Mirrors Random's `dwell_s`.
    pub pulses_per_level: Vec<u32>,
    /// When AUTO moves to the next level (default [`LadderAdvance::Count`]).
    /// Left out of the serialized schedule at its default, so a schedule
    /// written before it existed reads and writes the same bytes.
    #[serde(default, skip_serializing_if = "LadderAdvance::is_count")]
    pub advance: LadderAdvance,
    /// Pulses at one level without a pass (AUTO, [`LadderAdvance::OnPass`]),
    /// or failing pulses in a row at one MANUAL level, before a closed-loop
    /// run ends as not converged (default [`DEFAULT_MAX_PULSES_PER_LEVEL`]).
    /// Left out of the serialized schedule at its default.
    #[serde(
        default = "default_max_pulses_per_level",
        skip_serializing_if = "is_default_max_pulses_per_level"
    )]
    pub max_pulses_per_level: u32,
}

impl ShockLevelSchedule {
    /// The default AUTO schedule: `[-12, -9, -6, -3, 0]` dB, 1 pulse per level.
    pub fn auto_default() -> Self {
        ShockLevelSchedule {
            mode: ShockRunMode::Auto,
            start_db: DEFAULT_START_LEVEL_DB,
            table_db: DEFAULT_LEVEL_TABLE_DB.to_vec(),
            pulses_per_level: vec![DEFAULT_PULSES_PER_LEVEL],
            advance: LadderAdvance::Count,
            max_pulses_per_level: DEFAULT_MAX_PULSES_PER_LEVEL,
        }
    }

    /// A MANUAL schedule starting at `start_db` (default -12 dB).
    pub fn manual_default() -> Self {
        ShockLevelSchedule {
            mode: ShockRunMode::Manual,
            start_db: DEFAULT_START_LEVEL_DB,
            table_db: DEFAULT_LEVEL_TABLE_DB.to_vec(),
            pulses_per_level: vec![DEFAULT_PULSES_PER_LEVEL],
            advance: LadderAdvance::Count,
            max_pulses_per_level: DEFAULT_MAX_PULSES_PER_LEVEL,
        }
    }

    /// Pulses to fire at table level `i` (single-entry `pulses_per_level` applies to
    /// all). Mirrors Random's `dwell_for`.
    pub fn pulses_for(&self, i: usize) -> u32 {
        if self.pulses_per_level.is_empty() {
            0
        } else if self.pulses_per_level.len() == 1 {
            self.pulses_per_level[0]
        } else {
            self.pulses_per_level
                .get(i)
                .copied()
                .unwrap_or(*self.pulses_per_level.last().unwrap())
        }
    }

    /// The full level (last table entry, or 0 dB if the table is empty). Mirrors
    /// Random's `full_db`.
    pub fn full_db(&self) -> f64 {
        self.table_db.last().copied().unwrap_or(0.0)
    }

    /// Validate the schedule: the table must be non-empty and
    /// non-decreasing; a non-single `pulses_per_level` must match the table length;
    /// AUTO must have at least one pulse per level.
    pub fn validate(&self) -> Result<(), LadderError> {
        if self.table_db.is_empty() {
            return Err(LadderError::BadSchedule("level table is empty".into()));
        }
        for w in self.table_db.windows(2) {
            if w[1] < w[0] {
                return Err(LadderError::BadSchedule(format!(
                    "level table is not non-decreasing ({} then {})",
                    w[0], w[1]
                )));
            }
        }
        if self.pulses_per_level.is_empty() {
            return Err(LadderError::BadSchedule("pulses_per_level is empty".into()));
        }
        if self.pulses_per_level.len() != 1 && self.pulses_per_level.len() != self.table_db.len() {
            return Err(LadderError::BadSchedule(format!(
                "pulses_per_level length {} must be 1 or match table length {}",
                self.pulses_per_level.len(),
                self.table_db.len()
            )));
        }
        if self.mode == ShockRunMode::Auto
            && (0..self.table_db.len()).any(|i| self.pulses_for(i) == 0)
        {
            return Err(LadderError::BadSchedule(
                "AUTO requires at least one pulse per level".into(),
            ));
        }
        Ok(())
    }
}

/// The mutable run position on the ladder. Constructed from a validated
/// [`ShockLevelSchedule`]; ticked once per fired pulse. Pure state - no I/O.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShockLadderState {
    schedule: ShockLevelSchedule,
    /// Index into `table_db` of the current level (AUTO progression + jump target).
    level_index: usize,
    /// Pulses already fired at the current level (AUTO advance counter).
    pulses_at_level: u32,
    /// The MANUAL dB offset added on top of the table level for `+/- 1 dB` nudges.
    /// Zero in AUTO. Kept separate from the table so a jump resets it cleanly.
    manual_offset_db: f64,
    /// True once the run has walked off the top of the table (AUTO reached full and
    /// completed its pulses). The service uses this to schedule a clean stop.
    completed: bool,
    /// Pulses at the current level since its last passing pulse (or since the
    /// level was entered). Counted only by [`ShockLadderState::on_pulse_judged`].
    #[serde(default)]
    without_pass: u32,
}

/// What one judged pulse did to the ladder ([`ShockLadderState::on_pulse_judged`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LadderTick {
    /// The level stays.
    Held,
    /// AUTO moved to the next level.
    Advanced,
    /// AUTO completed its top level.
    Completed,
    /// `max_pulses_per_level` pulses at this level without a pass (AUTO with
    /// [`LadderAdvance::OnPass`]), or failing pulses in a row at this level
    /// (MANUAL): the run does not converge.
    NotConverged {
        /// The pulses counted against the limit.
        pulses: u32,
    },
}

impl ShockLadderState {
    /// Build a ladder from a schedule, validating it first. The initial level is
    /// `start_db` snapped to the nearest table index (AUTO) or held as a manual offset
    /// (MANUAL) so `current_level_db` starts at `start_db`.
    pub fn new(schedule: ShockLevelSchedule) -> Result<Self, LadderError> {
        schedule.validate()?;
        // Snap the start level to the table: find the nearest table index, then carry
        // any remainder as a manual offset so current_level_db == start_db exactly.
        let mut idx = 0usize;
        let mut best = f64::INFINITY;
        for (i, &lv) in schedule.table_db.iter().enumerate() {
            let d = (lv - schedule.start_db).abs();
            if d < best {
                best = d;
                idx = i;
            }
        }
        let manual_offset = schedule.start_db - schedule.table_db[idx];
        Ok(ShockLadderState {
            schedule,
            level_index: idx,
            pulses_at_level: 0,
            manual_offset_db: manual_offset,
            completed: false,
            without_pass: 0,
        })
    }

    /// The currently commanded level in dB (table level + manual offset).
    pub fn current_level_db(&self) -> f64 {
        self.schedule.table_db[self.level_index] + self.manual_offset_db
    }

    /// The current level's linear scale, `10^(level_db/20)`. The reference pulse is
    /// multiplied by this before firing.
    pub fn current_scale(&self) -> f64 {
        10.0_f64.powf(self.current_level_db() / 20.0)
    }

    /// The current table index.
    pub fn level_index(&self) -> usize {
        self.level_index
    }

    /// Pulses fired at the current level so far.
    pub fn pulses_at_level(&self) -> u32 {
        self.pulses_at_level
    }

    /// True once AUTO has completed the top level (the service schedules a clean
    /// stop). Always false in MANUAL.
    pub fn completed(&self) -> bool {
        self.completed
    }

    /// The schedule this ladder runs.
    pub fn schedule(&self) -> &ShockLevelSchedule {
        &self.schedule
    }

    /// Tick the ladder after one pulse fired at the current level.
    ///
    /// In AUTO: increments the pulse counter; when it reaches `pulses_for(level)` the
    /// ladder advances to the next table level (resetting the counter). Reaching the
    /// top and completing its pulses sets [`Self::completed`]. In MANUAL: only counts
    /// the pulse (no automatic progression); the operator drives the level with
    /// [`Self::step_up`] / [`Self::step_down`] / [`Self::jump_to_level`].
    pub fn on_pulse_fired(&mut self) {
        self.pulses_at_level = self.pulses_at_level.saturating_add(1);
        if self.schedule.mode != ShockRunMode::Auto {
            return;
        }
        let needed = self.schedule.pulses_for(self.level_index);
        if self.pulses_at_level >= needed {
            if self.level_index + 1 < self.schedule.table_db.len() {
                self.level_index += 1;
                self.pulses_at_level = 0;
            } else {
                // Completed the top level.
                self.completed = true;
            }
        }
    }

    /// Pulses at the current level since its last passing pulse, or since the
    /// level was entered.
    pub fn pulses_without_pass(&self) -> u32 {
        self.without_pass
    }

    /// Tick the ladder after one pulse fired at the current level and was
    /// judged, for a closed-loop run: the schedule's [`LadderAdvance`] and
    /// [`ShockLevelSchedule::max_pulses_per_level`] apply.
    ///
    /// - AUTO, [`LadderAdvance::Count`]: as [`Self::on_pulse_fired`]; no
    ///   per-level limit.
    /// - AUTO, [`LadderAdvance::OnPass`]: only a passing pulse counts toward the
    ///   level's pulse count; `max_pulses_per_level` pulses at the level without
    ///   a pass do not converge.
    /// - MANUAL: counts the pulse; `max_pulses_per_level` failing pulses in a
    ///   row at one level do not converge.
    ///
    /// [`Self::on_pulse_fired`] is the open loop's tick and is unchanged.
    pub fn on_pulse_judged(&mut self, passed: bool) -> LadderTick {
        let max = self.schedule.max_pulses_per_level.max(1);
        if passed {
            self.without_pass = 0;
        } else {
            self.without_pass = self.without_pass.saturating_add(1);
        }
        let before = self.level_index;
        match (self.schedule.mode, self.schedule.advance) {
            (ShockRunMode::Auto, LadderAdvance::Count) => {
                self.on_pulse_fired();
                if self.completed {
                    return LadderTick::Completed;
                }
                if self.level_index != before {
                    self.without_pass = 0;
                    return LadderTick::Advanced;
                }
                LadderTick::Held
            }
            (ShockRunMode::Auto, LadderAdvance::OnPass) => {
                if !passed {
                    if self.without_pass >= max {
                        return LadderTick::NotConverged {
                            pulses: self.without_pass,
                        };
                    }
                    return LadderTick::Held;
                }
                self.on_pulse_fired();
                if self.completed {
                    return LadderTick::Completed;
                }
                if self.level_index != before {
                    return LadderTick::Advanced;
                }
                LadderTick::Held
            }
            (ShockRunMode::Manual, _) => {
                self.pulses_at_level = self.pulses_at_level.saturating_add(1);
                if self.without_pass >= max {
                    return LadderTick::NotConverged {
                        pulses: self.without_pass,
                    };
                }
                LadderTick::Held
            }
        }
    }

    /// Nudge the commanded level up by [`MANUAL_STEP_DB`]. Allowed in any
    /// mode (the operator can trim even during an AUTO run); it moves the manual
    /// offset, leaving the table index and AUTO progression intact.
    pub fn step_up(&mut self) {
        self.manual_offset_db += MANUAL_STEP_DB;
        self.without_pass = 0;
    }

    /// Nudge the commanded level down by [`MANUAL_STEP_DB`].
    pub fn step_down(&mut self) {
        self.manual_offset_db -= MANUAL_STEP_DB;
        self.without_pass = 0;
    }

    /// Jump directly to table level `i`, clearing any manual offset and
    /// resetting the per-level pulse counter. Returns [`LadderError::BadLevelIndex`]
    /// if `i` is out of range.
    pub fn jump_to_level(&mut self, i: usize) -> Result<(), LadderError> {
        if i >= self.schedule.table_db.len() {
            return Err(LadderError::BadLevelIndex {
                index: i,
                len: self.schedule.table_db.len(),
            });
        }
        self.level_index = i;
        self.manual_offset_db = 0.0;
        self.pulses_at_level = 0;
        self.completed = false;
        self.without_pass = 0;
        Ok(())
    }
}

/// Errors from building or driving the ladder.
#[derive(Debug, Clone, PartialEq)]
pub enum LadderError {
    /// The schedule failed validation. Carries a note.
    BadSchedule(String),
    /// A jump target was out of the table range.
    BadLevelIndex { index: usize, len: usize },
}

impl std::fmt::Display for LadderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LadderError::BadSchedule(s) => write!(f, "invalid shock level schedule: {s}"),
            LadderError::BadLevelIndex { index, len } => {
                write!(
                    f,
                    "jump-to-level {index} out of range (table has {len} levels)"
                )
            }
        }
    }
}

impl std::error::Error for LadderError {}
