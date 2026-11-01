//! `hertzforge-sine` - Sine module core: profile grammar + deterministic feasibility.
//!
//! The [`profile`] module carries the sine profile grammar:
//!
//! - Rows are `(freq, quantity, value)` with quantity in {accel g pk,
//!   velocity in/s pk, displacement in pk-pk} plus optional per-row alarm/abort
//!   `(minus, plus)` dB bands over global defaults (1.5 / 3.0 dB symmetric).
//! - Segment `i` spans `[row_i.freq, row_{i+1}.freq)` and is governed by the LEFT
//!   row. Same-quantity neighbors interpolate LOG-LOG between the two values
//!   (equal values = the constant case); differing quantities hold the left row's
//!   constant value, and validation warns on an amplitude discontinuity above
//!   0.5 dB at the join (both sides converted to acceleration at the join
//!   frequency).
//! - Evaluation converts the governing quantity to acceleration via SHM:
//!   `a[in/s^2] = (2 pi f) * v = (2 pi f)^2 * (d_pp / 2)`, `a_g = a / G_TO_IN_S2`.
//! - Automatic crossovers between constant-valued adjacent segments of different
//!   quantities (Irvine formulas), with a snap helper that recomputes the boundary
//!   frequencies.
//! - Hardened TSV/CSV/clipboard import + CSV/TSV/JSON export mirroring
//!   `hertzforge-breakpoints` (header aliasing, RFC-4180 quoting, delimiter
//!   auto-detect, comment tolerance, exactly-one-quantity-column precedence rule,
//!   lossless JSON round-trip).
//!
//! The [`feasibility`] module is the deterministic (crest factor = 1) sine
//! feasibility assessment: per-frequency analytic maxima over the
//! profile band compared like-for-like against the `hertzforge-shaker-db` sine
//! ratings, reusing that crate's report TYPES only (no PSD calculator).
//!
//! The [`control`] module is the scalar per-tick control core: composite
//! amplitude, the sysID feedforward table + delta-form blend law, the notch
//! governor with the scalar limit-over-control arbitration, the drive guard +
//! analytic kinematic helpers, and the compression presets.
//!
//! The [`scheduler`] module carries the pure scheduling state machines:
//! [`scheduler::BidirSweep`] (with its pinned edge rules), dwell in seconds or
//! cycles, step-sine, the RSTD search/settle/capture/dwell orchestration, and the
//! level ladder.
//!
//! Pure library: no I/O beyond what serde does; no `unwrap`/`expect`/`panic!` in
//! library paths (validation returns errors).

pub mod control;
pub mod feasibility;
pub mod profile;
pub mod scheduler;
pub mod service;
pub mod sweep_raster;

pub use control::{
    composite_amplitude, tracked_displacement_in_pp, tracked_velocity_ips_pk, BlendConfig,
    BlendLaw, CompositeMode, CompressionPreset, ControlError, DriveGuard, DriveVerdict,
    FeedforwardConfig, FeedforwardTable, LimitChannelTick, NotchGovernor, NotchGovernorConfig,
};
pub use feasibility::assess_sine;
pub use profile::{
    accel_g_pk_of, crossover_hz, displacement_in_pp_from_accel, import_with_report, pct_to_db,
    velocity_ips_pk_from_accel, ProfileError, ProfileRow, ProfileWarning, Quantity, SineProfile,
    ValidationReport, DEFAULT_ABORT_DB, DEFAULT_ALARM_DB, G_TO_IN_S2, JOIN_DISCONTINUITY_WARN_DB,
};
pub use scheduler::{
    BidirSweep, BidirSweepConfig, DwellFor, DwellPlan, DwellRun, LadderConfig, LadderPlan,
    LadderProgress, RstdConfig, RstdDwellMode, RstdEvent, RstdPlan, RstdState, RungFor, StepSine,
    StepSineConfig, StepSineProgress, StepSpacing, SweepDirection, SweepProgress, SweepState,
    SweepTarget,
};
pub use service::config::{
    CaptureConfig, CaptureRule, ColaConfig, ColaWaveform, ConfigError, ControlChannel, ControlSet,
    LimitProfile, MetaField, SineMode, SineTestConfig,
};
pub use service::report::{
    ChannelRow, ChannelTrace, EventKind, EventLog, FeasibilityDigest, FrfDisplay, Honesty,
    LevelHistoryEntry, ReportData, RunSettings, RunState, SineStatus, TracePoint, TripKind,
    TripRecord,
};
pub use service::{
    RunMode, ServiceError, SineDisplayChannel, SineDisplaySnapshot, SineService,
    SineSnapshotPayload,
};
pub use sweep_raster::{resample_at, SweepRaster, SWEEP_RASTER_POINTS};

/// Re-export the hertzforge-dsp sweep-rate law so config authors have one import path
/// for the whole sine config surface (it appears in [`BidirSweepConfig::rate`]
/// and [`RstdConfig::rate`]).
pub use hertzforge_dsp::sweep::SweepRate;

/// Re-export the hertzforge-dsp tracking-bandwidth law, for the same reason as
/// [`SweepRate`]: it appears in [`SineTestConfig::tracking_bw`], so a caller
/// assembling a whole sine config had to depend on `hertzforge-dsp` for exactly one
/// enum. The daemon's wire -> config converter is the caller that made this
/// visible - a hardware-free orchestration crate should not need the DSP crate on
/// its dependency list to name a config field.
pub use hertzforge_dsp::tracking::BandwidthMode;
