//! The size limits and default guards of the first version.
//!
//! The cadence, budget, miss count and change caps are single-shaker Sine's,
//! applied per drive.

/// The most control channels a run controls.
pub const MAX_CONTROLS: usize = 24;

/// The most drives a run commands.
pub const MAX_DRIVES: usize = 12;

/// The most limit channels a run watches.
pub const MAX_LIMITS: usize = 8;

/// The most values a law may report on one call.
pub const MAX_LAW_VALUES: usize = hertzforge_sine::law::MAX_LAW_VALUES;

/// The highest drive ceiling or abort, volts peak.
pub const MAX_DRIVE_VPK: f64 = 10.0;

/// The lowest sample rate asked of the station, Hz.
pub const MIN_FS_HZ: f64 = 1000.0;

/// The highest sample rate asked of the station, Hz.
pub const MAX_FS_HZ: f64 = 204_800.0;

/// The default control tick, seconds.
pub const DEFAULT_TICK_S: f64 = hertzforge_sine::law::DEFAULT_TICK_S;

/// The shortest control tick for a built-in law, seconds.
pub const MIN_TICK_S: f64 = hertzforge_sine::law::MIN_TICK_S_BUILTIN;

/// The shortest control tick for a law file, seconds.
pub const MIN_TICK_S_LAW_FILE: f64 = hertzforge_sine::law::MIN_TICK_S_LAW_FILE;

/// The longest control tick, seconds.
pub const MAX_TICK_S: f64 = 1.0;

/// The default share of the tick one law call may take.
pub const DEFAULT_LAW_BUDGET_FRACTION: f64 = hertzforge_sine::law::DEFAULT_LAW_BUDGET_FRACTION;

/// The default count of unusable calls in a row that take the controlled stop.
pub const DEFAULT_LAW_MISSES: u32 = hertzforge_sine::law::DEFAULT_LAW_MISSES;

/// The default cap on each drive's magnitude change in one update, dB.
pub const DEFAULT_MAX_STEP_DB: f64 = hertzforge_sine::law::DEFAULT_MAX_STEP_DB;

/// The default cap on each drive's rise, dB per second.
pub const DEFAULT_MAX_RISE_DB_PER_S: f64 = hertzforge_sine::law::DEFAULT_MAX_RISE_DB_PER_S;

/// The default time a cap may bind without a break before the controlled
/// stop, seconds.
pub const DEFAULT_DIVERGENCE_S: f64 = hertzforge_sine::law::DEFAULT_DIVERGENCE_S;

/// The first drive vector's norm is at most the identification's inverse
/// drive at the start frequency and level plus this, dB.
pub const FIRST_DRIVE_MARGIN_DB: f64 = hertzforge_sine::law::FIRST_DRIVE_MARGIN_DB;

/// Without a trusted identification at the start frequency, the first drive
/// vector's norm is at most this times the ceilings' norm times the start
/// level.
pub const FIRST_DRIVE_UNTRUSTED_FRACTION: f64 =
    hertzforge_sine::law::FIRST_DRIVE_UNTRUSTED_FRACTION;

/// The longest a granted hold may last without a break, seconds.
pub const MAX_HOLD_S_BOUND: f64 = 2.0;

/// The lowest share of the scheduled sweep rate a granted slow may ask for.
pub const MIN_SLOW_FACTOR_BOUND: f64 = 0.1;
