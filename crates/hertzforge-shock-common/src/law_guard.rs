//! The guards a shock control law runs under, shared by classical shock and SRS.
//!
//! A shock law is called between pulses, never while one plays: once after
//! system identification (the seed) and once after each fired, measured and
//! judged pulse. The law proposes; the core disposes. This module holds the
//! pieces every shock module needs to run a law under the same rules:
//!
//! - [`LawGuards`]: the time budget of one call (a fraction of the rate
//!   model's minimum pulse spacing), how many unusable calls in a row stop the
//!   run, and the cap on pulses in a run.
//! - [`LawClock`] and [`MonotonicClock`]: the clock a call is timed on,
//!   injectable so a test makes a law "slow" without resting on the wall clock.
//! - [`LawFault`] and [`OutputRule`]: why a call was not used, with the stable
//!   names the wire and the records carry.
//! - [`MissCounter`]: unusable calls in a row, and when they stop the run.
//! - [`LawRecordRing`] and [`LawCallRecord`]: a bounded record of every call,
//!   drained between pulses into the run folder.
//! - [`LawValue`] and [`LawDescriptor`]: a law's reported values and its
//!   identity in the records.
//!
//! The arithmetic of each module's own output checks and change caps lives
//! with that module (classical shock in `hertzforge-shock-host`, SRS in
//! `hertzforge-srs`); what is here is what both shock modules share, the names
//! of every output rule included.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt;
use std::time::Instant;

/// Default time budget of one law call, as a fraction of the rate model's
/// minimum pulse spacing: 250 ms at the default 1.0 s floor, the same as the
/// rate model's compute floor.
pub const DEFAULT_LAW_BUDGET_FRACTION: f64 = 0.25;

/// Default number of unusable law calls in a row that stop the run.
pub const DEFAULT_LAW_MISSES: u32 = 3;

/// Default cap on the pulses one run may fire.
pub const DEFAULT_MAX_PULSES: u64 = 100;

/// The most values a law may report on one call.
pub const MAX_LAW_VALUES: usize = 8;

/// Default number of law calls the record ring keeps before the oldest is
/// dropped. The ring is drained after every pulse, so it holds one or two in
/// practice; the bound keeps a host that never drains it from growing.
pub const DEFAULT_LAW_RECORDS: usize = 256;

/// The guards on calling a shock law, the same for every law.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawGuards {
    /// Fraction of the rate model's minimum pulse spacing one law call may take
    /// before its output is not used. A law file's host deadline is the same
    /// number.
    pub budget_fraction: f64,
    /// Unusable law calls in a row (an error, a panic, a late call or a refused
    /// output) at which the run takes its controlled stop and ends `law_fault`.
    pub max_misses: u32,
    /// The most pulses one run may fire, whatever the schedule says.
    pub max_pulses: u64,
}

impl Default for LawGuards {
    fn default() -> Self {
        LawGuards {
            budget_fraction: DEFAULT_LAW_BUDGET_FRACTION,
            max_misses: DEFAULT_LAW_MISSES,
            max_pulses: DEFAULT_MAX_PULSES,
        }
    }
}

impl LawGuards {
    /// The time one call may take, seconds, for a minimum pulse spacing of
    /// `min_spacing_s`.
    pub fn call_budget_s(&self, min_spacing_s: f64) -> f64 {
        self.budget_fraction * min_spacing_s
    }

    /// Check the guards' ranges. The error names the field.
    pub fn validate(&self) -> Result<(), String> {
        if !(self.budget_fraction.is_finite()
            && self.budget_fraction > 0.0
            && self.budget_fraction <= 1.0)
        {
            return Err(format!(
                "law_budget must be above 0 and at most 1 (got {})",
                self.budget_fraction
            ));
        }
        if !(1..=100).contains(&self.max_misses) {
            return Err(format!(
                "law_misses must be 1 to 100 (got {})",
                self.max_misses
            ));
        }
        if self.max_pulses == 0 {
            return Err("max_pulses must be at least 1".into());
        }
        Ok(())
    }
}

/// The clock a law call is timed on. Monotonic seconds from an arbitrary
/// origin.
pub trait LawClock: Send + Sync {
    /// Seconds since this clock's origin. Never decreases.
    fn now_s(&self) -> f64;
}

/// The production clock: [`std::time::Instant`] from the moment it was built.
#[derive(Debug, Clone, Copy)]
pub struct MonotonicClock {
    origin: Instant,
}

impl MonotonicClock {
    /// A clock whose origin is now.
    pub fn new() -> Self {
        MonotonicClock {
            origin: Instant::now(),
        }
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl LawClock for MonotonicClock {
    fn now_s(&self) -> f64 {
        self.origin.elapsed().as_secs_f64()
    }
}

/// Which output rule a refused law output broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputRule {
    /// The output had the wrong number of values (a drive record not
    /// `record-len` samples long).
    WrongLength,
    /// The output held a value that is not a finite number.
    NonFinite,
    /// A reported value under a key the manifest does not declare.
    UndeclaredValue,
    /// More reported values than [`MAX_LAW_VALUES`].
    TooManyValues,
    /// The drive's peak rose more than the per-pulse peak cap over the last
    /// drive played.
    PeakRise,
    /// A bin's magnitude rose more than the per-pulse bin cap over the last
    /// drive played.
    BinRise,
    /// The seed's predicted response peak was over the first-drive bound.
    FirstDrive,
    /// A gain was zero or negative (an SRS law's gains are multiplicative
    /// and must be above zero).
    NotPositive,
    /// A gain rose more than the per-pulse gain cap (an SRS law).
    GainRise,
}

impl OutputRule {
    /// The stable wire and record name.
    pub fn as_str(self) -> &'static str {
        match self {
            OutputRule::WrongLength => "wrong_length",
            OutputRule::NonFinite => "non_finite",
            OutputRule::UndeclaredValue => "undeclared_value",
            OutputRule::TooManyValues => "too_many_values",
            OutputRule::PeakRise => "peak_rise",
            OutputRule::BinRise => "bin_rise",
            OutputRule::FirstDrive => "first_drive",
            OutputRule::NotPositive => "not_positive",
            OutputRule::GainRise => "gain_rise",
        }
    }
}

impl fmt::Display for OutputRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let words = match self {
            OutputRule::WrongLength => "the output had the wrong length",
            OutputRule::NonFinite => "the output held a value that is not a finite number",
            OutputRule::UndeclaredValue => "the law reported a value its manifest does not declare",
            OutputRule::TooManyValues => "the law reported more values than allowed",
            OutputRule::PeakRise => "the drive's peak rose more than the per-pulse cap",
            OutputRule::BinRise => "a drive bin rose more than the per-pulse cap",
            OutputRule::FirstDrive => {
                "the first drive's predicted response was over the first-drive bound"
            }
            OutputRule::NotPositive => "a gain was not above zero",
            OutputRule::GainRise => "a gain rose more than the per-pulse cap",
        };
        f.write_str(words)
    }
}

/// Why a law call was not used. Every one is a miss; [`MissCounter`] decides
/// when misses stop the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LawFault {
    /// The law reported that it could not compute an output.
    Failed(String),
    /// The law panicked (a native law) or trapped (a law file); the core caught
    /// it.
    Crashed(String),
    /// The call took longer than its budget.
    OverBudget {
        /// How long the call took, seconds.
        took_s: f64,
        /// The budget, seconds.
        budget_s: f64,
    },
    /// A law file asked for more memory than its cap.
    OverMemory,
    /// The core refused the output.
    BadOutput(OutputRule),
}

impl LawFault {
    /// The stable wire and record name: `error`, `panicked`, `over_budget`,
    /// `over_memory`, or the broken [`OutputRule`]'s name.
    pub fn reason(&self) -> &'static str {
        match self {
            LawFault::Failed(_) => "error",
            LawFault::Crashed(_) => "panicked",
            LawFault::OverBudget { .. } => "over_budget",
            LawFault::OverMemory => "over_memory",
            LawFault::BadOutput(rule) => rule.as_str(),
        }
    }
}

impl fmt::Display for LawFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LawFault::Failed(s) => write!(f, "the control law reported an error: {s}"),
            LawFault::Crashed(s) => write!(f, "the control law crashed: {s}"),
            LawFault::OverBudget { took_s, budget_s } => write!(
                f,
                "the control law took {:.1} ms, over its {:.1} ms budget",
                took_s * 1e3,
                budget_s * 1e3
            ),
            LawFault::OverMemory => {
                f.write_str("the control law asked for more memory than its cap")
            }
            LawFault::BadOutput(rule) => write!(f, "the control law's output was refused: {rule}"),
        }
    }
}

impl std::error::Error for LawFault {}

/// Unusable law calls in a row. A usable call resets the count; the run stops
/// when the count reaches the limit, and the law is not called again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissCounter {
    limit: u32,
    in_a_row: u32,
    total: u64,
}

impl MissCounter {
    /// A counter that trips at `limit` misses in a row (at least 1).
    pub fn new(limit: u32) -> Self {
        MissCounter {
            limit: limit.max(1),
            in_a_row: 0,
            total: 0,
        }
    }

    /// Count a miss. True when this miss reaches the limit: the run stops.
    pub fn miss(&mut self) -> bool {
        self.in_a_row = self.in_a_row.saturating_add(1);
        self.total = self.total.saturating_add(1);
        self.tripped()
    }

    /// Count a usable call: the count in a row starts again.
    pub fn used(&mut self) {
        self.in_a_row = 0;
    }

    /// Misses in a row now.
    pub fn in_a_row(&self) -> u32 {
        self.in_a_row
    }

    /// Misses this run.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// The limit.
    pub fn limit(&self) -> u32 {
        self.limit
    }

    /// Whether the misses in a row have reached the limit.
    pub fn tripped(&self) -> bool {
        self.in_a_row >= self.limit
    }
}

/// Which call of a law a record is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LawCall {
    /// The first call, after identification and before any pulse.
    Seed,
    /// A call after a fired, measured and judged pulse.
    Iterate,
}

/// One law call as recorded: which call, whether its output was used, how long
/// it took, and the module's own inputs and outputs in `payload`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawCallRecord<P> {
    /// Pulses fired before the call: 0 at the seed.
    pub pulse_index: u64,
    /// The seed or an iteration.
    pub call: LawCall,
    /// Why the output was not used; `None` when it was.
    pub miss: Option<LawFault>,
    /// How long the call took, microseconds.
    pub call_us: u64,
    /// The module's record of the call: its inputs, the output, what the core
    /// capped.
    pub payload: P,
}

/// A bounded record of law calls. The run pushes one entry per call and the
/// host drains it between pulses, after the pulse is judged and never during a
/// fire. When full, the oldest entry is dropped and counted.
#[derive(Debug, Clone, PartialEq)]
pub struct LawRecordRing<T> {
    entries: VecDeque<T>,
    capacity: usize,
    dropped: u64,
}

impl<T> LawRecordRing<T> {
    /// An empty ring holding at most `capacity` entries (at least 1).
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        LawRecordRing {
            entries: VecDeque::with_capacity(capacity.min(DEFAULT_LAW_RECORDS)),
            capacity,
            dropped: 0,
        }
    }

    /// Add an entry, dropping the oldest when the ring is full.
    pub fn push(&mut self, entry: T) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.entries.push_back(entry);
    }

    /// Take every entry, oldest first, leaving the ring empty.
    pub fn drain(&mut self) -> Vec<T> {
        self.entries.drain(..).collect()
    }

    /// Entries held now.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ring holds nothing.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The most entries the ring holds.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Entries dropped, unread, because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl<T> Default for LawRecordRing<T> {
    fn default() -> Self {
        Self::new(DEFAULT_LAW_RECORDS)
    }
}

/// One value a law reports on a call, under a key its manifest declares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawValue {
    /// The declared key.
    pub key: String,
    /// The value.
    pub value: f64,
}

impl LawValue {
    /// The value `value` under `key`.
    pub fn new(key: impl Into<String>, value: f64) -> Self {
        LawValue {
            key: key.into(),
            value,
        }
    }
}

/// A law as the run's records name it (`law.json`) and the report's one line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawDescriptor {
    /// The catalog id: a built-in law's name, or an imported law file's
    /// SHA-256.
    pub id: String,
    /// The law's name from its manifest.
    pub name: String,
    /// The law's version from its manifest.
    pub version: String,
    /// The law file's SHA-256; `None` for a built-in law.
    pub sha256: Option<String>,
    /// The parameter values the run uses, every declared one, by key.
    pub params: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_guards_are_the_plan_numbers_and_validate() {
        let g = LawGuards::default();
        assert_eq!(g.budget_fraction, 0.25);
        assert_eq!(g.max_misses, 3);
        assert_eq!(g.max_pulses, 100);
        g.validate().unwrap();
        assert!((g.call_budget_s(1.0) - 0.25).abs() < 1e-15);
    }

    #[test]
    fn each_guard_out_of_range_is_refused_by_name() {
        for bad in [0.0, -0.1, 1.01, f64::NAN] {
            let g = LawGuards {
                budget_fraction: bad,
                ..LawGuards::default()
            };
            assert!(g.validate().unwrap_err().contains("law_budget"), "{bad}");
        }
        for bad in [0, 101] {
            let g = LawGuards {
                max_misses: bad,
                ..LawGuards::default()
            };
            assert!(g.validate().unwrap_err().contains("law_misses"), "{bad}");
        }
        let g = LawGuards {
            max_pulses: 0,
            ..LawGuards::default()
        };
        assert!(g.validate().unwrap_err().contains("max_pulses"));
    }

    #[test]
    fn misses_trip_at_the_limit_and_a_usable_call_starts_the_count_again() {
        let mut m = MissCounter::new(3);
        assert!(!m.miss());
        assert!(!m.miss());
        m.used();
        assert_eq!(m.in_a_row(), 0);
        assert!(!m.miss());
        assert!(!m.miss());
        assert!(m.miss(), "the third in a row trips");
        assert!(m.tripped());
        assert_eq!(m.total(), 5);
    }

    #[test]
    fn a_zero_limit_is_held_at_one() {
        let mut m = MissCounter::new(0);
        assert_eq!(m.limit(), 1);
        assert!(m.miss());
    }

    #[test]
    fn the_ring_keeps_the_newest_and_counts_what_it_dropped() {
        let mut r = LawRecordRing::new(2);
        r.push(1);
        r.push(2);
        r.push(3);
        assert_eq!(r.len(), 2);
        assert_eq!(r.dropped(), 1);
        assert_eq!(r.drain(), vec![2, 3]);
        assert!(r.is_empty());
        assert_eq!(r.dropped(), 1, "draining does not reset the dropped count");
    }

    #[test]
    fn fault_reasons_are_the_wire_names() {
        assert_eq!(LawFault::Failed("x".into()).reason(), "error");
        assert_eq!(LawFault::Crashed("x".into()).reason(), "panicked");
        assert_eq!(
            LawFault::OverBudget {
                took_s: 0.3,
                budget_s: 0.25
            }
            .reason(),
            "over_budget"
        );
        assert_eq!(LawFault::OverMemory.reason(), "over_memory");
        assert_eq!(
            LawFault::BadOutput(OutputRule::FirstDrive).reason(),
            "first_drive"
        );
        assert_eq!(
            LawFault::BadOutput(OutputRule::NotPositive).reason(),
            "not_positive"
        );
        assert_eq!(
            LawFault::BadOutput(OutputRule::GainRise).reason(),
            "gain_rise"
        );
        // The serde name of each rule is its wire name.
        for rule in [
            OutputRule::WrongLength,
            OutputRule::NonFinite,
            OutputRule::UndeclaredValue,
            OutputRule::TooManyValues,
            OutputRule::PeakRise,
            OutputRule::BinRise,
            OutputRule::FirstDrive,
            OutputRule::NotPositive,
            OutputRule::GainRise,
        ] {
            assert_eq!(
                serde_json::to_value(rule).unwrap(),
                serde_json::Value::String(rule.as_str().into())
            );
        }
    }

    #[test]
    fn the_monotonic_clock_never_runs_backwards() {
        let c = MonotonicClock::new();
        let a = c.now_s();
        let b = c.now_s();
        assert!(b >= a && a >= 0.0);
    }
}
