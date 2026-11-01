//! Pulse-train scheduler + rate model. This is the core landing here as scaffolding,
//! hardware-blocked markers included.
//!
//! # What a pulse train is
//!
//! A train fires N pulses, either with a fixed inter-pulse DELAY or as fast as the
//! hardware allows ([`Spacing::MaxRate`]). The gunfire mode (MIL-STD-810 Method 519.8
//! Procedure IV) is a repeated-pulse train at a rounds-per-minute rate; the
//! [`gunfire_spacing`] helper converts a rate to a [`Spacing::Delay`].
//!
//! # The rate model is a floor, not a target
//!
//! The true limiters on how fast pulses can repeat are NOT the DAQ. They are, in
//! rough order: armature/coil cooling, fixture ring-down settling, SRS/capture
//! compute, and (on Path A only) the on-device per-pulse compensation recompute (the
//! decoded OEM pulses were 22-92 s apart!), plus read-cadence sustainment.
//! [`RateModel::effective_min_spacing`] combines these floors into the minimum legal
//! inter-pulse spacing for a given [`crate::path::DrivePath`], and [`TrainSpec::plan`]
//! REFUSES a spec that asks for spacing below the floor.
//!
//! # Hardware-blocked - PATH-AWARE since 2026-08-08
//!
//! This is a DELEGATED-PATH unknown: on the delegated controller the
//! write-triggers-fire semantic is undecoded (the decoded -12 dB repeats may be
//! re-asserts, not fires), so a
//! multi-pulse train on that path cannot be claimed to fire on real hardware.
//! [`TrainPlan::hardware_blocked`] carries that marker.
//!
//! It used to be set for EVERY multi-pulse train on EVERY path
//! (`hardware_blocked: self.count > 1`), which put a delegated-path caveat on the
//! Path-B host trains the caveat does not describe: Path B writes its own drive
//! samples into a DAC and there is no trigger semantic to decode. The marker is now
//! `count > 1 && path.is_delegated()`, which is what the unknown actually covers.
//!
//! **Consequence, stated rather than hidden:** the marker is UNREACHABLE through
//! [`TrainSpec::plan`] today, because a multi-pulse train on the delegated path is
//! already refused one check earlier as [`TrainError::AtRateNotOnPath`]
//! (`pulse_trains_at_rate` is `false` on Path A). The field is kept because it is on
//! the report and, once the delegated path can carry an at-rate train, it is the
//! honest place for the delegated-path caveat to reappear. What is gone is a marker that was
//! shipping to operators on every host train and describing nothing.
//!
//! At-rate trains (rounds/min) are HostDac-only:
//! [`crate::path::PathCapabilities::pulse_trains_at_rate`] is the enforcement point,
//! checked by [`TrainSpec::plan`].

use crate::path::DrivePath;
use serde::{Deserialize, Serialize};

/// How pulses in a train are spaced.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Spacing {
    /// A fixed inter-pulse delay, seconds (edge-to-edge, pulse start to pulse start).
    Delay(f64),
    /// Fire as fast as the hardware allows: the scheduler uses
    /// [`RateModel::effective_min_spacing`] as the realized spacing.
    MaxRate,
}

/// A pulse-train specification: how many pulses and how they are spaced.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrainSpec {
    /// Number of pulses to fire (>= 1).
    pub count: u32,
    /// Inter-pulse spacing.
    pub spacing: Spacing,
}

impl TrainSpec {
    /// A single pulse (count 1). Spacing is irrelevant for one pulse; stored as a
    /// zero delay.
    pub fn single() -> Self {
        TrainSpec {
            count: 1,
            spacing: Spacing::Delay(0.0),
        }
    }

    /// A train of `count` pulses at a fixed `delay_s` between pulse starts.
    pub fn delayed(count: u32, delay_s: f64) -> Self {
        TrainSpec {
            count,
            spacing: Spacing::Delay(delay_s),
        }
    }

    /// A train of `count` pulses fired as fast as the hardware allows.
    pub fn max_rate(count: u32) -> Self {
        TrainSpec {
            count,
            spacing: Spacing::MaxRate,
        }
    }

    /// Plan this train for a given drive path against a rate model.
    ///
    /// Validates and REFUSES a spec whose realized spacing would be below
    /// the model floor, or a spec with zero pulses. For a [`Spacing::Delay`] a delay
    /// below the floor is [`TrainError::SpacingBelowFloor`]; for [`Spacing::MaxRate`]
    /// the realized spacing IS the floor (never below it). An at-rate train (any
    /// multi-pulse train) on a path whose capabilities forbid at-rate trains is
    /// [`TrainError::AtRateNotOnPath`] (at-rate is HostDac-only).
    ///
    /// On success returns a [`TrainPlan`] carrying the realized spacing, the total
    /// train duration, and the hardware-blocked marker.
    pub fn plan(&self, path: DrivePath, model: &RateModel) -> Result<TrainPlan, TrainError> {
        if self.count == 0 {
            return Err(TrainError::EmptyTrain);
        }
        let floor = model.effective_min_spacing(path);

        // A multi-pulse train IS an at-rate/repeated train; enforce the path rule
        // (at-rate trains are HostDac-only). A single pulse is always
        // allowed (no repetition).
        if self.count > 1 && !path.capabilities().pulse_trains_at_rate {
            return Err(TrainError::AtRateNotOnPath { path });
        }

        let realized_spacing_s = match self.spacing {
            Spacing::Delay(d) => {
                if !d.is_finite() || d < 0.0 {
                    return Err(TrainError::BadSpacing(d));
                }
                // A single pulse ignores spacing. A multi-pulse train must clear the
                // floor.
                if self.count > 1 && d < floor {
                    return Err(TrainError::SpacingBelowFloor {
                        requested_s: d,
                        floor_s: floor,
                    });
                }
                d
            }
            // MaxRate realizes exactly the floor - never below it.
            Spacing::MaxRate => floor,
        };

        // Total duration: (count - 1) inter-pulse gaps. (The pulse widths themselves
        // are small vs the spacing and are added by the caller if needed.)
        let total_duration_s = realized_spacing_s * (self.count.saturating_sub(1) as f64);

        Ok(TrainPlan {
            count: self.count,
            realized_spacing_s,
            total_duration_s,
            floor_s: floor,
            // PATH-AWARE (see the module docs): the write-triggers-fire
            // semantic is undecoded on the DELEGATED device, so a multi-pulse train
            // THERE cannot be claimed to fire on hardware. Path B writes its own
            // samples into a DAC and has no trigger semantic to decode, so a host
            // train never carried this caveat honestly. A single pulse is not a
            // "train" on either path and never carries the marker.
            hardware_blocked: self.count > 1 && path.is_delegated(),
            path,
        })
    }
}

/// The realized plan for a [`TrainSpec`] on a path.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrainPlan {
    /// Number of pulses.
    pub count: u32,
    /// The realized inter-pulse spacing, seconds (>= the model floor).
    pub realized_spacing_s: f64,
    /// Total train duration, seconds (`(count - 1) * realized_spacing_s`).
    pub total_duration_s: f64,
    /// The rate-model floor that constrained this plan, seconds.
    pub floor_s: f64,
    /// HARDWARE-BLOCKED marker: a multi-pulse train **on the DELEGATED
    /// path** is Sim/mock-transport only until the write-triggers-fire semantic is verified
    /// against an inbound capture. A service/UI must surface this so an operator does
    /// not believe an at-rate train will fire on real hardware.
    ///
    /// Host (Path B) trains never carried the caveat honestly and no longer set it;
    /// see the module docs for why the flag is currently unreachable through
    /// [`TrainSpec::plan`].
    pub hardware_blocked: bool,
    /// The path this plan is for.
    pub path: DrivePath,
}

/// The rate model: the floors that set the minimum legal inter-pulse
/// spacing. Every floor is a separate physical constraint; the effective floor is the
/// max of the ones that apply on the given path.
///
/// All floors are in seconds. The device-recompute floor applies on Path A ONLY (the
/// OEM device recomputes compensation per pulse; the decoded pulses were 22-92 s
/// apart). `read_cadence_ok` is a gate, not a duration: if the read cadence cannot be
/// sustained the model is not usable and [`RateModel::effective_min_spacing`] returns
/// [`f64::INFINITY`] (refuse everything) rather than a plausible-looking number.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RateModel {
    /// Armature/coil cooling floor, seconds (limiter 1).
    pub cooling_floor_s: f64,
    /// Fixture ring-down settling floor, seconds (limiter 2).
    pub settling_floor_s: f64,
    /// SRS/capture compute floor, seconds (limiter 3).
    pub compute_floor_s: f64,
    /// On-device per-pulse compensation-recompute floor, seconds. PATH A ONLY
    /// (limiter 4): the delegated device recomputes compensation each pulse; on
    /// Path B (host owns compensation, computed ahead of the train) this floor does
    /// NOT apply.
    pub device_recompute_floor_s: f64,
    /// Whether the read cadence can be sustained (limiter 5). If false the model
    /// refuses every spacing (returns [`f64::INFINITY`]).
    pub read_cadence_ok: bool,
}

impl RateModel {
    /// A conservative default rate model. The numbers are placeholders pending the
    /// hardware characterization; they are deliberately non-trivial (seconds, not
    /// microseconds) so a naive "fire at 1 kHz" spec is refused rather than silently
    /// accepted. Documented as a placeholder: real floors come from measured cooling /
    /// ring-down / compute-time data.
    pub fn conservative_default() -> Self {
        RateModel {
            cooling_floor_s: 1.0,
            settling_floor_s: 0.5,
            compute_floor_s: 0.25,
            // The OEM decoded pulses were 22-92 s apart on the delegated device; a
            // conservative single-figure recompute floor stands in until it is measured.
            device_recompute_floor_s: 20.0,
            read_cadence_ok: true,
        }
    }

    /// The effective minimum inter-pulse spacing for `path`, seconds: the MAX of the
    /// applicable floors (the binding constraint). The device-recompute floor is
    /// included ONLY on [`DrivePath::Delegated`]. If the read cadence cannot be
    /// sustained the model refuses everything ([`f64::INFINITY`]).
    pub fn effective_min_spacing(&self, path: DrivePath) -> f64 {
        if !self.read_cadence_ok {
            return f64::INFINITY;
        }
        let mut floor = self
            .cooling_floor_s
            .max(self.settling_floor_s)
            .max(self.compute_floor_s);
        if path.is_delegated() {
            floor = floor.max(self.device_recompute_floor_s);
        }
        floor.max(0.0)
    }
}

/// Gunfire helper (MIL-STD-810 Method 519.8 Procedure IV): convert a firing rate in
/// rounds per minute to the inter-pulse [`Spacing::Delay`] of `60 / rate` seconds.
///
/// # Procedure IV
///
/// The classical-module fit for gunfire is Procedure IV: a repeated-pulse train at a
/// rounds-per-minute rate (burst = rounds x bursts; spacing `60 / rate` s). The
/// spectral sine-on-random method (Procedure III) is a RANDOM-module concern and is
/// explicitly out of scope here.
///
/// **At-rate trains are HostDac-only:** the returned spacing is only
/// FIREABLE if [`crate::path::PathCapabilities::pulse_trains_at_rate`] is true for the
/// path, which [`TrainSpec::plan`] enforces; on Path A this helper still computes the
/// spacing (for display/planning) but planning the train will fail
/// [`TrainError::AtRateNotOnPath`].
///
/// Returns [`TrainError::BadSpacing`] for a non-positive or non-finite rate.
pub fn gunfire_spacing(rounds_per_min: f64) -> Result<Spacing, TrainError> {
    if !rounds_per_min.is_finite() || rounds_per_min <= 0.0 {
        return Err(TrainError::BadSpacing(rounds_per_min));
    }
    Ok(Spacing::Delay(60.0 / rounds_per_min))
}

/// Errors from planning a pulse train.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrainError {
    /// A train with zero pulses.
    EmptyTrain,
    /// A spacing/rate value was negative or non-finite.
    BadSpacing(f64),
    /// The requested delay is below the rate-model floor (the model
    /// refuses rates below the floors).
    SpacingBelowFloor { requested_s: f64, floor_s: f64 },
    /// An at-rate (multi-pulse) train was requested on a path that does not support
    /// at-rate trains (at-rate is HostDac-only).
    AtRateNotOnPath { path: DrivePath },
}

impl std::fmt::Display for TrainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrainError::EmptyTrain => write!(f, "pulse train has zero pulses"),
            TrainError::BadSpacing(v) => write!(f, "invalid pulse spacing/rate: {v}"),
            TrainError::SpacingBelowFloor {
                requested_s,
                floor_s,
            } => write!(
                f,
                "requested inter-pulse spacing {requested_s:.4} s is below the rate-model floor \
                 {floor_s:.4} s (cooling / settling / compute / device-recompute limit)"
            ),
            TrainError::AtRateNotOnPath { path } => write!(
                f,
                "at-rate pulse trains are not available on {path:?} (at-rate trains are \
                 HostDac-only; the delegated device recomputes compensation per pulse)"
            ),
        }
    }
}

impl std::error::Error for TrainError {}
