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
//! override-ack, the Random feature-4 posture. Full eigen-shaker-db integration
//! is a UI-layer follow-up (the panel can copy a shaker record's ratings into
//! these ceilings without an engine change).

use serde::{Deserialize, Serialize};

use crate::reference::WaveformKinematics;
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
        let peak = drive.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
        self.check_peak_value(peak)
    }

    /// [`TwrLimits::check_drive_peak`] against a peak already in hand - the
    /// level verbs know theirs without playing anything, because scaling every
    /// bin by `s` scales the time record by exactly `s`.
    pub fn check_peak_value(&self, peak: f64) -> Option<String> {
        let ceiling = self.max_drive_vpk?;
        if peak > ceiling {
            return Some(format!(
                "drive peak {peak:.4} V exceeds the max drive ceiling {ceiling:.4} V \
                 (peak volts, measured on the realized drive record): REFUSED, not clipped \
                 - clipping would distort the drive and the law would then converge around \
                 the distortion"
            ));
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
        let mut abort: Option<String> = None;
        for (i, (&t, &a)) in target.iter().zip(achieved.iter()).enumerate() {
            let e = (a - t).abs();
            worst = worst.max(e / peak);
            if e > alarm_w {
                alarm = true;
            }
            if e > abort_w {
                consec += 1;
                if consec >= self.envelope_debounce_samples && abort.is_none() {
                    abort = Some(format!(
                        "response envelope abort: |error| > {:.0}% of target peak for {} \
                         consecutive samples (at sample {})",
                        self.envelope_abort_frac * 100.0,
                        consec,
                        i
                    ));
                }
            } else {
                consec = 0;
            }
        }
        if abort.is_none() {
            if let Some(lim) = self.response_rms_limit {
                let rms = eigen_dsp::stats::rms(achieved);
                if rms > lim {
                    abort = Some(format!("response RMS {rms:.4} exceeds the limit {lim:.4}"));
                }
            }
        }
        ResponseOutcome {
            worst_frac: worst,
            alarm,
            abort,
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
        if let Some(lim) = self.drive_rms_limit {
            let rms = eigen_dsp::stats::rms(drive);
            if rms > lim {
                return Some(format!(
                    "drive RMS {rms:.4} exceeds the limit {lim:.4} (pre-play check)"
                ));
            }
        }
        None
    }
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
        let rms = eigen_dsp::stats::rms(&d);
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
