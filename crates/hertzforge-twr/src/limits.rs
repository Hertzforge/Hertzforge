//! Abort + limit logic and the pre-arm feasibility gate.
//!
//! # Response abort envelope (point-by-point, the DP/Crystal shape)
//!
//! The tolerance band around the target is CONSTANT-WIDTH relative to the
//! target's peak: sample `i` violates at `|y[i] - r_L[i]| > frac * peak|r_L|`.
//! Alarm and abort are two fractions of the same band; the abort requires
//! `debounce_samples` CONSECUTIVE violations so a single noise spike cannot
//! kill a run. Checked on every play (iteration or scheduled playback).
//!
//! # Drive limits
//!
//! - Per-sample hard clip at `drive_clip` (clipped count is reported per play -
//!   the loop then corrects the REALIZED clipped drive, the Random-module
//!   clip-then-measure posture).
//! - Drive RMS ceiling `drive_rms_limit` - exceeding it aborts BEFORE the play
//!   (predictive: the drive record is known before it is played).
//!
//! # Feasibility
//!
//! Config-level kinematic ceilings (peak accel / peak velocity / pk-pk
//! displacement of the FULL-LEVEL prepared reference) with a hard block +
//! override-ack, the Random feature-4 posture. Full hertzforge-shaker-db integration
//! is a UI-layer follow-up (the panel can copy a shaker record's ratings into
//! these ceilings without an engine change).

use serde::{Deserialize, Serialize};

use crate::reference::WaveformKinematics;
use crate::trip::TripKind;
use crate::TwrError;

/// The drive ceiling every module ships with, peak volts. The standing
/// decision: the UI defaults to it, CLEARING it refuses, and the service will
/// not run without one.
pub const DEFAULT_MAX_DRIVE_VPK: f64 = 5.0;

/// Limit + abort configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TwrLimits {
    /// Alarm at `|y - r_L| > alarm_frac * peak|r_L|` (latched flag, run
    /// continues). Default 0.25.
    pub envelope_alarm_frac: f64,
    /// Abort at `|y - r_L| > abort_frac * peak|r_L|` for `debounce_samples`
    /// consecutive samples. Default 0.5.
    pub envelope_abort_frac: f64,
    /// Consecutive violating samples before the envelope abort fires. Default 8.
    pub envelope_debounce_samples: usize,
    /// Achieved-record RMS ceiling (EU); exceeded -> abort. Optional.
    pub response_rms_limit: Option<f64>,
    /// Per-sample drive hard clip (drive units). Optional.
    pub drive_clip: Option<f64>,
    /// Drive-record RMS ceiling (drive units); exceeded -> abort BEFORE play.
    ///
    /// **This is NOT the drive ceiling.** A TWR reference is a TRANSIENT with a
    /// crest factor of 5-10, so an RMS ceiling of 5 authorizes 25-50 V of peak.
    /// The authorization is [`TwrLimits::max_drive_vpk`]; this is an additional
    /// energy guard and defaults to off.
    pub drive_rms_limit: Option<f64>,
    /// THE DRIVE CEILING: maximum PEAK volts of the realized drive record.
    ///
    /// Checked after the inverse filter has produced the record and before it
    /// reaches the player, and it REFUSES - it never clips. Clipping would
    /// distort silently and the law would then correct the clipped drive by
    /// construction, so the run would converge around its own distortion.
    ///
    /// `None` means the operator CLEARED the ceiling, and a cleared ceiling
    /// refuses to run (`validate`): the ceiling is the authorization, so no
    /// ceiling is no authorization. The default is 5.0 V, the same number every
    /// other module ships.
    ///
    /// Shock's profile-time projection (`peak_eu * 10^(level/20) *
    /// volts_per_eu`) is deliberately NOT imported: TWR's drive is the output
    /// of an inverse filter, and a plant notch makes it arbitrarily larger than
    /// the reference, so a ceiling computed from the reference peak would bless
    /// a run whose first corrected drive is 20x that.
    pub max_drive_vpk: Option<f64>,
    /// Divergence debounce: consecutive rises before abort.
    pub divergence_n: usize,
    /// Relative rise per iteration that counts as "rising". Default 0.10.
    pub divergence_tol_frac: f64,
    /// Pre-arm kinematic ceilings.
    pub feasibility: FeasibilityCeilings,
}

impl Default for TwrLimits {
    fn default() -> Self {
        TwrLimits {
            envelope_alarm_frac: 0.25,
            envelope_abort_frac: 0.5,
            envelope_debounce_samples: 8,
            response_rms_limit: None,
            drive_clip: None,
            drive_rms_limit: None,
            max_drive_vpk: Some(DEFAULT_MAX_DRIVE_VPK),
            divergence_n: 3,
            divergence_tol_frac: 0.10,
            feasibility: FeasibilityCeilings::default(),
        }
    }
}

impl TwrLimits {
    pub fn validate(&self) -> Result<(), TwrError> {
        if self.envelope_alarm_frac.is_nan()
            || self.envelope_abort_frac.is_nan()
            || self.envelope_alarm_frac <= 0.0
            || self.envelope_abort_frac <= 0.0
        {
            return Err(TwrError::Config("envelope fractions must be > 0".into()));
        }
        if self.envelope_alarm_frac > self.envelope_abort_frac {
            return Err(TwrError::Config(
                "envelope alarm fraction must not exceed the abort fraction".into(),
            ));
        }
        if self.envelope_debounce_samples == 0 {
            return Err(TwrError::Config(
                "envelope_debounce_samples must be >= 1".into(),
            ));
        }
        if self.divergence_n == 0 {
            return Err(TwrError::Config("divergence_n must be >= 1".into()));
        }
        if self.divergence_tol_frac.is_nan() || self.divergence_tol_frac < 0.0 {
            return Err(TwrError::Config("divergence_tol_frac must be >= 0".into()));
        }
        for (name, v) in [
            ("response_rms_limit", self.response_rms_limit),
            ("drive_clip", self.drive_clip),
            ("drive_rms_limit", self.drive_rms_limit),
            ("max_drive_vpk", self.max_drive_vpk),
        ] {
            if let Some(x) = v {
                if x.is_nan() || x <= 0.0 {
                    return Err(TwrError::Config(format!("{name} must be > 0")));
                }
            }
        }
        // The ceiling is the AUTHORIZATION, so its absence is not "unlimited",
        // it is "unauthorized". Every other limit here is optional; this one
        // refuses.
        if self.max_drive_vpk.is_none() {
            return Err(TwrError::Config(format!(
                "max_drive_vpk is cleared: the peak-drive ceiling is the authorization to \
                 drive, so a run cannot start without one (default {DEFAULT_MAX_DRIVE_VPK} V)"
            )));
        }
        Ok(())
    }

    /// THE CEILING CHECK, on a realized drive record. Returns the named
    /// refusal - carrying BOTH numbers - when the record's peak exceeds
    /// [`TwrLimits::max_drive_vpk`].
    ///
    /// Peak, not RMS, and on the REALIZED record, not a projection: see the
    /// field docs for why neither substitution is safe here.
    pub fn check_drive_peak(&self, drive: &[f64]) -> Option<String> {
        self.drive_peak_trip(drive).map(|t| t.message)
    }

    /// [`TwrLimits::check_drive_peak`] with the trip's numbers: the realized
    /// record's peak and the ceiling.
    pub fn drive_peak_trip(&self, drive: &[f64]) -> Option<LimitTrip> {
        let peak = drive.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
        self.peak_value_trip(peak, false)
    }

    /// [`TwrLimits::check_drive_peak`] against a peak already in hand - the
    /// level verbs know theirs without playing anything, because scaling every
    /// bin by `s` scales the time record by exactly `s`.
    pub fn check_peak_value(&self, peak: f64) -> Option<String> {
        self.peak_value_trip(peak, false).map(|t| t.message)
    }

    /// [`TwrLimits::check_peak_value`] with the trip's numbers. `predicted`
    /// marks a peak found on a projection, before anything played.
    pub fn peak_value_trip(&self, peak: f64, predicted: bool) -> Option<LimitTrip> {
        let ceiling = self.max_drive_vpk?;
        if peak > ceiling {
            return Some(LimitTrip {
                kind: TripKind::DriveVpk {
                    vpk: peak,
                    ceiling_vpk: ceiling,
                    predicted,
                },
                message: format!(
                    "drive peak {peak:.4} V exceeds the max drive ceiling {ceiling:.4} V \
                     (peak volts, measured on the realized drive record): REFUSED, not \
                     clipped - clipping would distort the drive and the law would then \
                     converge around the distortion"
                ),
            });
        }
        None
    }

    /// Check one play against the envelope + response RMS. Returns the outcome
    /// (worst violation fraction, latched alarm, abort reason if any).
    pub fn check_response(&self, target: &[f64], achieved: &[f64]) -> ResponseOutcome {
        let peak = target.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
        if peak <= 0.0 {
            return ResponseOutcome::default();
        }
        let alarm_w = self.envelope_alarm_frac * peak;
        let abort_w = self.envelope_abort_frac * peak;
        let mut worst = 0.0f64;
        let mut alarm = false;
        let mut consec = 0usize;
        // The trip is decided once; the abort sentence is derived from it, so
        // the two cannot disagree.
        let mut trip: Option<LimitTrip> = None;
        for (i, (&t, &a)) in target.iter().zip(achieved.iter()).enumerate() {
            let e = (a - t).abs();
            worst = worst.max(e / peak);
            if e > alarm_w {
                alarm = true;
            }
            if e > abort_w {
                consec += 1;
                if consec >= self.envelope_debounce_samples && trip.is_none() {
                    trip = Some(LimitTrip {
                        kind: TripKind::Envelope {
                            in_play: false,
                            // The whole record's worst, filled in below.
                            worst_frac: 0.0,
                            abort_frac: self.envelope_abort_frac,
                            samples: consec,
                            at_sample: i,
                        },
                        message: format!(
                            "response envelope abort: |error| > {:.0}% of target peak for {} \
                             consecutive samples (at sample {})",
                            self.envelope_abort_frac * 100.0,
                            consec,
                            i
                        ),
                    });
                }
            } else {
                consec = 0;
            }
        }
        if let Some(LimitTrip {
            kind: TripKind::Envelope { worst_frac, .. },
            ..
        }) = trip.as_mut()
        {
            *worst_frac = worst;
        }
        if trip.is_none() {
            if let Some(lim) = self.response_rms_limit {
                let rms = hertzforge_dsp::stats::rms(achieved);
                if rms > lim {
                    trip = Some(LimitTrip {
                        kind: TripKind::ResponseRms { rms, limit: lim },
                        message: format!("response RMS {rms:.4} exceeds the limit {lim:.4}"),
                    });
                }
            }
        }
        ResponseOutcome {
            worst_frac: worst,
            alarm,
            abort: trip.as_ref().map(|t| t.message.clone()),
            trip,
        }
    }

    /// Clip the drive record in place; returns the clipped-sample count.
    pub fn apply_drive_clip(&self, drive: &mut [f64]) -> usize {
        let Some(clip) = self.drive_clip else {
            return 0;
        };
        let mut n = 0usize;
        for v in drive.iter_mut() {
            if v.abs() > clip {
                *v = v.signum() * clip;
                n += 1;
            }
        }
        n
    }

    /// Pre-play drive check (RMS ceiling). Returns the abort reason if tripped.
    pub fn check_drive(&self, drive: &[f64]) -> Option<String> {
        self.drive_rms_trip(drive).map(|t| t.message)
    }

    /// [`TwrLimits::check_drive`] with the trip's numbers.
    pub fn drive_rms_trip(&self, drive: &[f64]) -> Option<LimitTrip> {
        if let Some(lim) = self.drive_rms_limit {
            let rms = hertzforge_dsp::stats::rms(drive);
            if rms > lim {
                return Some(LimitTrip {
                    kind: TripKind::DriveRms {
                        vrms: rms,
                        limit_vrms: lim,
                    },
                    message: format!(
                        "drive RMS {rms:.4} exceeds the limit {lim:.4} (pre-play check)"
                    ),
                });
            }
        }
        None
    }
}

/// A limit that tripped: its kind with its numbers, and the sentence the run
/// log and the abort reason carry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitTrip {
    pub kind: TripKind,
    pub message: String,
}

/// The per-play response-check outcome.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseOutcome {
    /// Worst `|error| / peak(target)` seen this play.
    pub worst_frac: f64,
    /// The alarm band was entered at least once (latched by the service).
    pub alarm: bool,
    /// Abort reason, if the abort band + debounce (or the RMS limit) tripped.
    pub abort: Option<String>,
    /// What tripped, with its numbers; set exactly when `abort` is, which is
    /// its sentence.
    pub trip: Option<LimitTrip>,
}

/// Pre-arm kinematic ceilings. `None` = not evaluated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct FeasibilityCeilings {
    /// Peak |acceleration| ceiling, waveform EU (nominally g).
    pub peak_accel: Option<f64>,
    /// Peak |velocity| ceiling, in/s.
    pub peak_velocity_ips: Option<f64>,
    /// Peak-to-peak displacement ceiling, inches.
    pub disp_pkpk_in: Option<f64>,
}

/// One feasibility comparison row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeasibilityCheck {
    pub name: String,
    pub value: f64,
    pub limit: f64,
    pub pass: bool,
}

/// The pre-arm feasibility verdict.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FeasibilityReport {
    pub checks: Vec<FeasibilityCheck>,
    /// True when any evaluated check failed: arming hard-blocks until an
    /// override-ack (the Random feature-4 posture).
    pub blocked: bool,
}

impl FeasibilityCeilings {
    /// Compare the FULL-LEVEL reference kinematics against the ceilings.
    pub fn assess(&self, kin: &WaveformKinematics) -> FeasibilityReport {
        let mut checks = Vec::new();
        let mut push = |name: &str, value: f64, limit: Option<f64>| {
            if let Some(l) = limit {
                checks.push(FeasibilityCheck {
                    name: name.to_string(),
                    value,
                    limit: l,
                    pass: value <= l,
                });
            }
        };
        push("peak acceleration", kin.peak_accel, self.peak_accel);
        push(
            "peak velocity (ips)",
            kin.peak_velocity_ips,
            self.peak_velocity_ips,
        );
        push(
            "displacement pk-pk (in)",
            kin.disp_pkpk_in,
            self.disp_pkpk_in,
        );
        let blocked = checks.iter().any(|c| !c.pass);
        FeasibilityReport { checks, blocked }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> TwrLimits {
        TwrLimits::default()
    }

    #[test]
    fn envelope_alarm_latches_and_abort_debounces() {
        let l = limits();
        let n = 100;
        let target = vec![1.0f64; n];
        // One-sample spike into the abort band: alarm yes, abort no (debounce).
        let mut a = target.clone();
        a[50] = 1.9;
        let out = l.check_response(&target, &a);
        assert!(out.alarm);
        assert!(out.abort.is_none());
        // Nine consecutive samples deep in the abort band: abort.
        let mut a = target.clone();
        for v in a.iter_mut().skip(40).take(9) {
            *v = 2.0;
        }
        let out = l.check_response(&target, &a);
        assert!(out.abort.is_some(), "{out:?}");
        assert!(out.worst_frac >= 1.0 - 1e-12);
    }

    /// The typed trips carry today's sentences byte for byte, and their
    /// numbers.
    #[test]
    fn the_typed_trips_carry_the_same_sentences_and_their_numbers() {
        let mut l = limits();
        l.max_drive_vpk = Some(1.0);
        let d = vec![0.0, 2.0, -0.5];
        let t = l.drive_peak_trip(&d).unwrap();
        assert_eq!(
            t.message,
            "drive peak 2.0000 V exceeds the max drive ceiling 1.0000 V (peak volts, measured \
             on the realized drive record): REFUSED, not clipped - clipping would distort the \
             drive and the law would then converge around the distortion"
        );
        assert_eq!(
            t.kind,
            TripKind::DriveVpk {
                vpk: 2.0,
                ceiling_vpk: 1.0,
                predicted: false
            }
        );
        assert_eq!(l.check_drive_peak(&d), Some(t.message.clone()));
        assert_eq!(l.check_peak_value(2.0), Some(t.message.clone()));
        let p = l.peak_value_trip(2.0, true).unwrap();
        assert_eq!(p.message, t.message);
        assert!(matches!(
            p.kind,
            TripKind::DriveVpk {
                predicted: true,
                ..
            }
        ));
        assert!(l.drive_peak_trip(&[0.5]).is_none());

        l.drive_rms_limit = Some(0.5);
        let r = l.drive_rms_trip(&d).unwrap();
        assert_eq!(
            r.message,
            "drive RMS 1.1902 exceeds the limit 0.5000 (pre-play check)"
        );
        assert_eq!(l.check_drive(&d), Some(r.message.clone()));
        match r.kind {
            TripKind::DriveRms { vrms, limit_vrms } => {
                assert_eq!(limit_vrms, 0.5);
                assert_eq!(vrms, hertzforge_dsp::stats::rms(&d));
            }
            k => panic!("{k:?}"),
        }

        let target = vec![1.0f64; 100];
        let mut a = target.clone();
        for v in a.iter_mut().skip(40).take(9) {
            *v = 2.0;
        }
        let out = limits().check_response(&target, &a);
        let e = out.trip.clone().unwrap();
        assert_eq!(
            e.message,
            "response envelope abort: |error| > 50% of target peak for 8 consecutive samples \
             (at sample 47)"
        );
        assert_eq!(out.abort, Some(e.message.clone()));
        assert_eq!(
            e.kind,
            TripKind::Envelope {
                in_play: false,
                worst_frac: 1.0,
                abort_frac: 0.5,
                samples: 8,
                at_sample: 47
            }
        );
        let clean = limits().check_response(&target, &target);
        assert!(clean.trip.is_none() && clean.abort.is_none());

        let mut l = limits();
        l.response_rms_limit = Some(0.5);
        let out = l.check_response(&target, &target);
        let t = out.trip.unwrap();
        assert_eq!(t.message, "response RMS 1.0000 exceeds the limit 0.5000");
        assert_eq!(out.abort, Some(t.message));
        assert_eq!(
            t.kind,
            TripKind::ResponseRms {
                rms: 1.0,
                limit: 0.5
            }
        );
    }

    #[test]
    fn response_rms_limit_trips() {
        let mut l = limits();
        l.response_rms_limit = Some(0.5);
        let target = vec![1.0f64; 10];
        let achieved = vec![1.0f64; 10]; // rms 1.0 > 0.5, but envelope is clean
        let out = l.check_response(&target, &achieved);
        assert!(out.abort.unwrap().contains("response RMS"));
    }

    #[test]
    fn drive_clip_counts_and_clips() {
        let mut l = limits();
        l.drive_clip = Some(1.0);
        let mut d = vec![0.5, -2.0, 1.5, 0.9];
        let n = l.apply_drive_clip(&mut d);
        assert_eq!(n, 2);
        assert_eq!(d, vec![0.5, -1.0, 1.0, 0.9]);
        assert_eq!(limits().apply_drive_clip(&mut d), 0);
    }

    /// The two ceilings are not interchangeable, and this is the arithmetic
    /// that says why. A TWR reference is a transient: this record
    /// has an RMS of 1 and a peak of 8, so an RMS ceiling of 2 blesses it while
    /// the 5 V peak ceiling refuses it. Implementing "max drive 5 V" as
    /// `drive_rms_limit = 5.0` would have authorized 40 V into the 9263.
    #[test]
    fn drive_rms_limit_is_not_the_drive_ceiling() {
        let mut d = vec![0.0f64; 128];
        d[7] = 8.0;
        d[23] = -8.0;
        let rms = hertzforge_dsp::stats::rms(&d);
        assert!((rms - 1.0).abs() < 1e-12, "fixture rms {rms}");

        let mut l = limits();
        l.drive_rms_limit = Some(2.0);
        l.max_drive_vpk = Some(5.0);
        assert!(
            l.check_drive(&d).is_none(),
            "the RMS guard passes this record - that is the trap"
        );
        let refusal = l
            .check_drive_peak(&d)
            .expect("the peak ceiling must refuse");
        assert!(
            refusal.contains("8.0000"),
            "names the commanded peak: {refusal}"
        );
        assert!(refusal.contains("5.0000"), "names the ceiling: {refusal}");
        assert!(
            refusal.contains("REFUSED, not clipped"),
            "says what it does NOT do: {refusal}"
        );
        // ...and the record itself is untouched by asking.
        assert_eq!(d[7], 8.0);
    }

    /// A cleared ceiling is not "unlimited", it is "unauthorized".
    #[test]
    fn a_cleared_drive_ceiling_refuses_to_validate() {
        let mut l = limits();
        assert_eq!(l.max_drive_vpk, Some(DEFAULT_MAX_DRIVE_VPK));
        assert!(l.validate().is_ok());
        l.max_drive_vpk = None;
        let e = l.validate().unwrap_err().to_string();
        assert!(e.contains("max_drive_vpk is cleared"), "{e}");
        assert!(e.contains("authorization"), "{e}");
        l.max_drive_vpk = Some(-1.0);
        assert!(l.validate().is_err());
    }

    #[test]
    fn drive_rms_check_is_predictive() {
        let mut l = limits();
        l.drive_rms_limit = Some(0.1);
        assert!(l.check_drive(&[1.0; 8]).is_some());
        assert!(l.check_drive(&[0.01; 8]).is_none());
    }

    #[test]
    fn feasibility_blocks_on_any_failed_ceiling() {
        let kin = WaveformKinematics {
            peak_accel: 12.0,
            peak_velocity_ips: 40.0,
            disp_pkpk_in: 0.6,
            rms: 3.0,
        };
        let c = FeasibilityCeilings {
            peak_accel: Some(20.0),
            peak_velocity_ips: Some(70.0),
            disp_pkpk_in: Some(0.5), // fails
        };
        let r = c.assess(&kin);
        assert_eq!(r.checks.len(), 3);
        assert!(r.blocked);
        // No ceilings -> nothing evaluated, never blocked.
        let r = FeasibilityCeilings::default().assess(&kin);
        assert!(r.checks.is_empty() && !r.blocked);
    }

    #[test]
    fn validation_rejects_inverted_bands() {
        let mut l = limits();
        l.envelope_alarm_frac = 0.9; // above abort 0.5
        assert!(l.validate().is_err());
        assert!(limits().validate().is_ok());
    }
}
