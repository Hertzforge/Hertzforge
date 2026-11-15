//! The size limits of the first version.

/// The most control channels a run controls.
pub const MAX_CONTROLS: usize = 16;

/// The most drives a run commands.
pub const MAX_DRIVES: usize = 8;

/// The longest record, samples. Every record length is a power of two.
pub const MAX_RECORD_LEN: usize = 16384;

/// The shortest record, samples.
pub const MIN_RECORD_LEN: usize = 64;

/// The most values a law may report on one call.
pub const MAX_LAW_VALUES: usize = hertzforge_shock_common::law_guard::MAX_LAW_VALUES;
