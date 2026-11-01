//! The TWR run configuration - everything the operator edits, serializable for
//! setups save/recall.

use serde::{Deserialize, Serialize};

use crate::inverse::InverseConfig;
use crate::law::LawConfig;
use crate::limits::TwrLimits;
use crate::reference::ReferenceWaveform;
use crate::schedule::{RunSchedule, Section, TwrLevelSchedule};
use crate::sysid::SysIdConfig;
use crate::TwrError;

/// Reference preparation, applied in the documented order:
/// resample -> detrend -> highpass -> scale -> taper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrepConfig {
    /// Resample the reference to the player rate when they differ. When false,
    /// a rate mismatch is a configuration ERROR (explicit is better than a
    /// silent linear resample).
    pub auto_resample: bool,
    /// Remove the mean (`Some(false)`) or the linear trend (`Some(true)`).
    pub detrend: Option<bool>,
    /// Drift high-pass corner, Hz (at or below the correction band low edge).
    pub highpass_hz: Option<f64>,
    /// EU rescale factor (import-unit correction). 1.0 = none.
    pub scale: f64,
    /// Cosine end-taper fraction per end, (0, 0.5].
    pub taper_frac: Option<f64>,
}

impl Default for PrepConfig {
    fn default() -> Self {
        PrepConfig {
            auto_resample: true,
            detrend: Some(false),
            highpass_hz: Some(5.0),
            scale: 1.0,
            taper_frac: Some(0.02),
        }
    }
}

/// A user-defined metadata field (run number, test article, ... - the house
/// setups-metadata pattern).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetaField {
    pub key: String,
    pub label: String,
    pub value: String,
}

/// Capture behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureRules {
    /// Start capturing automatically at the first play.
    pub auto_start: bool,
    /// Run folder name under the data root.
    pub run_name: String,
}

impl Default for CaptureRules {
    fn default() -> Self {
        CaptureRules {
            auto_start: false,
            run_name: "twr_run".to_string(),
        }
    }
}

/// The complete TWR run configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TwrConfig {
    /// The imported reference (target) waveform.
    pub reference: ReferenceWaveform,
    pub prep: PrepConfig,
    /// The slice of the PREPARED reference to replicate (None = all of it).
    pub section: Option<Section>,
    /// Quiet pad before the section on the working grid (drive pre-ring room).
    /// Samples.
    pub pad_pre: usize,
    /// Quiet pad after the section (ring-down room). Samples.
    pub pad_post: usize,
    pub sysid: SysIdConfig,
    pub inverse: InverseConfig,
    pub law: LawConfig,
    pub schedule: TwrLevelSchedule,
    pub run: RunSchedule,
    /// Keep correcting during scheduled playback (default false - a frozen,
    /// converged drive is deterministic report evidence).
    pub adapt_during_playback: bool,
    pub limits: TwrLimits,
    /// High-pass corner for the feasibility kinematics preview, Hz.
    pub kinematics_hp_hz: f64,
    /// PARK after a PASSING system ID, before the first correction iteration,
    /// so the operator can look at the |H| and the coherence before anything
    /// climbs the ladder. Default ON.
    ///
    /// # It is authored here and HELD by the caller, deliberately
    ///
    /// [`crate::TwrService::advance`] performs exactly ONE unit per call and
    /// owns no loop, so it has nowhere to park: a park is a decision not to
    /// call `advance()` again yet, and only whoever is driving the units can
    /// make it. The daemon's `TwrHost` holds it today (its pump exits at the
    /// park and `ProceedAfterReview` respawns it, the shape `sine.rs` and
    /// `srs.rs` already use); a local run holds it in its own run thread.
    ///
    /// So this field is the AUTHORING surface and not a service switch, and
    /// that distinction is written down rather than left to be discovered:
    /// a reader who greps the service for `review_after_sysid` and finds no
    /// consumer must not conclude the flag is dead.
    ///
    /// **This is not the coherence hold.** [`crate::TwrState::CoherenceHold`]
    /// is the gate FAILING and has no release verb at all; this park is the
    /// gate PASSING and waiting for a human. Turning this off does not weaken
    /// that one.
    #[serde(default = "default_review_after_sysid")]
    pub review_after_sysid: bool,
    pub capture: CaptureRules,
    pub metadata: Vec<MetaField>,
}

/// The serde default for [`TwrConfig::review_after_sysid`]: `true`.
///
/// A default is REQUIRED and it is `true`: setups saved before the field
/// existed must load, and they must load into the SAFE posture. Defaulting a
/// missing review hold to "off" would silently un-gate every stored setup on
/// the first run after an upgrade.
fn default_review_after_sysid() -> bool {
    true
}

impl TwrConfig {
    /// A default-shaped config around a reference (UI fills the rest).
    pub fn for_reference(reference: ReferenceWaveform) -> Self {
        TwrConfig {
            reference,
            prep: PrepConfig::default(),
            section: None,
            pad_pre: 512,
            pad_post: 1024,
            sysid: SysIdConfig::default(),
            inverse: InverseConfig::default(),
            law: LawConfig::default(),
            schedule: TwrLevelSchedule::default(),
            run: RunSchedule::Once,
            adapt_during_playback: false,
            limits: TwrLimits::default(),
            kinematics_hp_hz: 5.0,
            review_after_sysid: default_review_after_sysid(),
            capture: CaptureRules::default(),
            metadata: vec![
                MetaField {
                    key: "run_number".into(),
                    label: "Run Number".into(),
                    value: String::new(),
                },
                MetaField {
                    key: "test_article".into(),
                    label: "Test Article".into(),
                    value: String::new(),
                },
                MetaField {
                    key: "serial_number".into(),
                    label: "SN".into(),
                    value: String::new(),
                },
                MetaField {
                    key: "operator".into(),
                    label: "Operator".into(),
                    value: String::new(),
                },
            ],
        }
    }

    /// Validate the whole config against the player rate. Cross-checks:
    /// the LAW band must sit INSIDE the SYSID band (G outside the identified
    /// band is extrapolation, never corrected there).
    pub fn validate(&self, player_fs_hz: f64) -> Result<(), TwrError> {
        self.reference
            .validate()
            .map_err(|e| TwrError::Waveform(e.to_string()))?;
        if !self.prep.auto_resample && (self.reference.fs_hz - player_fs_hz).abs() > 1e-9 {
            return Err(TwrError::Config(format!(
                "reference rate {} Hz != player rate {} Hz and auto_resample is off - \
                 resample the reference explicitly",
                self.reference.fs_hz, player_fs_hz
            )));
        }
        if !(self.prep.scale.is_finite() && self.prep.scale != 0.0) {
            return Err(TwrError::Config(
                "prep scale must be finite non-zero".into(),
            ));
        }
        if let Some(f) = self.prep.taper_frac {
            if !(f > 0.0 && f <= 0.5) {
                return Err(TwrError::Config("taper_frac outside (0, 0.5]".into()));
            }
        }
        if let Some(hp) = self.prep.highpass_hz {
            if !(hp > 0.0 && hp <= self.law.f_lo_hz) {
                return Err(TwrError::Config(format!(
                    "prep highpass {hp} Hz must be in (0, law f_lo {}]",
                    self.law.f_lo_hz
                )));
            }
        }
        self.sysid.validate(player_fs_hz)?;
        self.inverse.validate()?;
        self.law.validate(player_fs_hz)?;
        if self.law.f_lo_hz < self.sysid.f_lo_hz || self.law.f_hi_hz > self.sysid.f_hi_hz {
            return Err(TwrError::Config(format!(
                "law band [{}, {}] must sit inside the sysid band [{}, {}] - the inverse is \
                 extrapolation outside the identified band",
                self.law.f_lo_hz, self.law.f_hi_hz, self.sysid.f_lo_hz, self.sysid.f_hi_hz
            )));
        }
        self.schedule.validate()?;
        self.run.validate()?;
        self.limits.validate()?;
        if self.kinematics_hp_hz.is_nan() || self.kinematics_hp_hz <= 0.0 {
            return Err(TwrError::Config("kinematics_hp_hz must be > 0".into()));
        }
        if self.capture.run_name.is_empty() {
            return Err(TwrError::Config("capture run_name is empty".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::ReferenceWaveform;

    fn cfg() -> TwrConfig {
        let r = ReferenceWaveform::from_samples("t", 8192.0, vec![0.5; 8192]).unwrap();
        TwrConfig::for_reference(r)
    }

    #[test]
    fn default_config_validates() {
        cfg().validate(8192.0).unwrap();
    }

    #[test]
    fn law_band_must_sit_inside_sysid_band() {
        let mut c = cfg();
        c.law.f_hi_hz = 3000.0; // sysid default hi = 2000
        c.sysid.f_hi_hz = 2000.0;
        assert!(c.validate(8192.0).is_err());
    }

    #[test]
    fn rate_mismatch_without_auto_resample_is_an_error() {
        let mut c = cfg(); // reference at 8192 Hz
        c.prep.auto_resample = false;
        assert!(c.validate(16_384.0).is_err());
        c.prep.auto_resample = true;
        assert!(c.validate(16_384.0).is_ok());
        // The rate is checked UPWARD here on purpose: a DOWNWARD mismatch also
        // passes this validation and is then refused by the prep pipeline's
        // resampler, which is the one authority on "no anti-alias filter"
        // (`reference::tests::resample_up_keeps_shape_and_down_refuses_by_name`).
        // Duplicating that rule here would make two.
    }

    #[test]
    fn prep_highpass_above_law_lo_rejected() {
        let mut c = cfg();
        c.prep.highpass_hz = Some(100.0); // law f_lo default 20
        assert!(c.validate(8192.0).is_err());
    }

    #[test]
    fn config_round_trips_json() {
        let c = cfg();
        let j = serde_json::to_string(&c).unwrap();
        let back: TwrConfig = serde_json::from_str(&j).unwrap();
        assert_eq!(c, back);
    }

    /// **THE POST-SYSID REVIEW HOLD DEFAULTS ON, and a setup saved before the
    /// field existed loads into the SAFE posture.**
    ///
    /// The second half is the one worth a test: `#[serde(default)]` on a `bool`
    /// would have defaulted to `false`, silently un-gating every stored setup
    /// on the first run after the upgrade. The named default is what makes an
    /// ABSENT field mean "hold", not "go".
    ///
    /// RED-MUTATION: change the attribute to a bare `#[serde(default)]` and the
    /// absent-field assertion reds.
    #[test]
    fn the_review_hold_defaults_on_including_for_a_setup_that_predates_it() {
        assert!(cfg().review_after_sysid, "the shipped default is ON");

        let mut j: serde_json::Value = serde_json::to_value(cfg()).unwrap();
        j.as_object_mut()
            .expect("a config is an object")
            .remove("review_after_sysid")
            .expect("the field was there to remove");
        let old: TwrConfig = serde_json::from_value(j).expect("an older setup must still load");
        assert!(
            old.review_after_sysid,
            "an ABSENT review flag is a HOLD, never a go"
        );

        // ANTI-VACUITY: an explicit `false` still crosses as false, so the
        // default above is a default and not a hard-coded answer.
        let mut j: serde_json::Value = serde_json::to_value(cfg()).unwrap();
        j["review_after_sysid"] = serde_json::Value::Bool(false);
        let bypassed: TwrConfig = serde_json::from_value(j).unwrap();
        assert!(!bypassed.review_after_sysid);
    }
}
