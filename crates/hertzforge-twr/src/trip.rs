//! Why the core ended a run itself: the protection that tripped, with its
//! numbers, so a host maps every end by its kind and never by parsing the
//! sentence in [`crate::TwrStatus::abort_reason`].
//!
//! A run the core ends itself is [`crate::TwrState::Aborted`] with a
//! [`TwrTrip`]. A run ended by the operator's [`crate::TwrService::abort`] is
//! `Aborted` with no trip; a stop is [`crate::TwrState::Stopped`].
//!
//! Some kinds have no producer yet: [`TripKind::Alignment`],
//! [`TripKind::LawFault`], [`TripKind::FirstDrive`] and the in-play form of
//! [`TripKind::Envelope`] belong to the guards a changeable control law runs
//! under, and are named here so the set a host maps is complete.

use hertzforge_shock_common::law_guard::LawFault;
use serde::{Deserialize, Serialize};

/// What tripped, and the trip's facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TripKind {
    /// The response left the abort band for the debounce count of samples in a
    /// row.
    Envelope {
        /// Found during the play from a block as it arrived, rather than after
        /// the whole record had played.
        in_play: bool,
        /// The worst `|response - target| / peak|target|` over what was judged.
        worst_frac: f64,
        /// The abort band, the same fraction of the target's peak.
        abort_frac: f64,
        /// Samples in a row outside the band when it tripped (the debounce).
        samples: usize,
        /// The record sample it tripped at.
        at_sample: usize,
    },
    /// A drive record's peak over the drive ceiling. The drive is refused, never
    /// clipped.
    DriveVpk {
        /// The drive's peak, volts.
        vpk: f64,
        /// The ceiling, volts peak.
        ceiling_vpk: f64,
        /// Found on a projection before the drive was played (a level step's
        /// exact scaling, or a prediction), so nothing was driven for it.
        predicted: bool,
    },
    /// A drive record's rms over the drive rms limit, before it played.
    DriveRms {
        /// The drive's rms, volts.
        vrms: f64,
        /// The limit, volts rms.
        limit_vrms: f64,
    },
    /// A played response's rms over the response rms limit.
    ResponseRms {
        /// The response's rms, control units.
        rms: f64,
        /// The limit, control units.
        limit: f64,
    },
    /// The trusted bins cover less of the reference's spectral energy than the
    /// floor, so the error could never fall under `error_floor_pct`.
    Coverage {
        /// Fraction of the reference's energy on trusted bins.
        coverage: f64,
        /// The floor it fell under.
        min_band_energy: f64,
        /// The error that much uncovered energy leaves, percent:
        /// `100 sqrt(1 - coverage)`.
        error_floor_pct: f64,
    },
    /// The rms error rose on `plays` correction plays in a row at one level,
    /// above the gate.
    Divergence {
        /// The rises in a row that tripped it.
        plays: usize,
        /// The error of the last of them, percent.
        error_pct: f64,
    },
    /// A level spent its correction plays without meeting the gate, under the
    /// abort policy.
    NotConverged {
        /// The correction plays spent at the level.
        iterations: u32,
        /// The error of the last of them, percent.
        error_pct: f64,
        /// The gate, percent.
        gate_pct: f64,
    },
    /// A play's fitted lag drifted from the identification's by more than the
    /// limit; nothing was corrected from that play.
    Alignment {
        /// The play's fitted lag, frames.
        lag_frames: f64,
        /// The identification's lag, frames.
        identification_lag_frames: f64,
        /// `lag_frames - identification_lag_frames`.
        drift_frames: f64,
        /// The largest drift allowed, frames.
        limit_frames: f64,
    },
    /// The control law gave no usable drive `consecutive` times in a row, or
    /// could not be built or seeded.
    LawFault {
        /// Why the last call was not used.
        reason: LawFault,
        /// Unusable calls in a row.
        consecutive: u32,
    },
    /// The law's first drive predicted a response peak over its bound; nothing
    /// played.
    FirstDrive {
        /// The seed's predicted response peak, control units.
        predicted_peak: f64,
        /// The level target's peak, control units.
        target_peak: f64,
        /// The bound, times the target's peak.
        ratio_limit: f64,
    },
    /// The delivery path returned no response record, so the play could not be
    /// judged. A fault of the path, not a protection.
    NoResponse,
}

impl TripKind {
    /// The stable record name.
    pub fn as_str(&self) -> &'static str {
        match self {
            TripKind::Envelope { .. } => "envelope",
            TripKind::DriveVpk { .. } => "drive_vpk",
            TripKind::DriveRms { .. } => "drive_rms",
            TripKind::ResponseRms { .. } => "response_rms",
            TripKind::Coverage { .. } => "coverage",
            TripKind::Divergence { .. } => "divergence",
            TripKind::NotConverged { .. } => "not_converged",
            TripKind::Alignment { .. } => "alignment",
            TripKind::LawFault { .. } => "law_fault",
            TripKind::FirstDrive { .. } => "first_drive",
            TripKind::NoResponse => "no_response",
        }
    }
}

/// A trip: what tripped, which play it judged, and the level the run was at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TwrTrip {
    /// Flattened, so a trip serializes as `{"kind", <its facts>, "play",
    /// "at_level_db"}`.
    #[serde(flatten)]
    pub kind: TripKind,
    /// The index of the play the trip judged (its row in
    /// [`crate::TwrService::plays`]). `None` when the trip came at
    /// identification, or refused a drive before it was played.
    pub play: Option<u64>,
    /// The commanded level when it tripped, dB.
    pub at_level_db: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trip_serializes_by_its_kind_with_its_facts() {
        let t = TwrTrip {
            kind: TripKind::DriveVpk {
                vpk: 0.2,
                ceiling_vpk: 0.15,
                predicted: true,
            },
            play: None,
            at_level_db: 0.0,
        };
        let j = serde_json::to_value(&t).unwrap();
        assert_eq!(j["kind"], "drive_vpk");
        assert_eq!(j["predicted"], true);
        assert_eq!(j["ceiling_vpk"], 0.15);
        assert_eq!(j["play"], serde_json::Value::Null);
        let back: TwrTrip = serde_json::from_value(j).unwrap();
        assert_eq!(back, t);
        assert_eq!(TripKind::NoResponse.as_str(), "no_response");
        let law = TripKind::LawFault {
            reason: LawFault::OverMemory,
            consecutive: 3,
        };
        assert_eq!(law.as_str(), "law_fault");
        let j = serde_json::to_value(&law).unwrap();
        let back: TripKind = serde_json::from_value(j).unwrap();
        assert_eq!(back, law);
    }
}
