//! What a MIMO Shock law sees after a pulse and what it returns.
//!
//! Mirrors `mimo-shock-inputs`, `control-verdict` and `mimo-shock-outputs` of
//! `wit/control-law/shock-mimo.wit`. The core checks the inputs it hands a law
//! ([`PulseInputs::validate`]) and the outputs a law returns
//! ([`PulseOutputs::validate`]); every protection of the drive (ceilings, the
//! growth cap, the predicted response) is the core's, after these.

use crate::limits::MAX_LAW_VALUES;
use crate::setup::{Setup, Target};
use hertzforge_shock_common::law_guard::LawValue;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The core's judgement of one control on the last pulse.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ControlVerdict {
    /// Inside the control's tolerance at its level.
    pub passed: bool,
    /// The worst deviation from the target, dB: the waveform error for a
    /// waveform target (after the control's delay is removed), the largest SRS
    /// deviation for an SRS target.
    pub error_db: f64,
    /// The control's measured delay against its target, s.
    pub delay_s: f64,
    /// Measured peak over the target's peak at the played level.
    pub peak_ratio: f64,
}

/// Everything a law sees after a pulse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PulseInputs {
    /// Pulses fired so far, 1 for the first iteration.
    pub pulse_index: u64,
    /// The level the last pulse was played at, dB.
    pub level_db: f64,
    /// The level the core has scheduled for the next pulse, dB.
    pub next_level_db: f64,
    /// The drives as played, after the core's limits: `drives * record_len`
    /// values, drive after drive, volts.
    pub drives: Vec<f64>,
    /// The measured responses: `controls * record_len` values, control after
    /// control, EU.
    pub responses: Vec<f64>,
    /// The measured SRS of every control on the target's natural frequencies,
    /// control after control. Present for an SRS target only.
    pub srs: Option<Vec<f64>>,
    /// The core's judgement of each control, in control order.
    pub verdicts: Vec<ControlVerdict>,
    /// The core scaled the last pulse down (a drive ceiling or a limit).
    pub limited: bool,
}

impl PulseInputs {
    /// Every control passed: the joint verdict the ladder is fed.
    pub fn all_passed(&self) -> bool {
        !self.verdicts.is_empty() && self.verdicts.iter().all(|v| v.passed)
    }
}

/// What a law returns for the next pulse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PulseOutputs {
    /// The next drives, at the next level, volts: `drives * record_len` finite
    /// values, drive after drive.
    pub drives: Vec<f64>,
    /// The delay the law expects at each control, s, one per control.
    pub delays_s: Option<Vec<f64>>,
    /// At most [`MAX_LAW_VALUES`] values, each under a declared key.
    pub values: Vec<LawValue>,
}

/// The rule a [`PulseInputs`] broke: a caller's error, not the law's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputsRule {
    /// `pulse_index` is 0 (the seed takes no inputs).
    PulseIndex,
    /// A level is not finite.
    Level,
    /// `drives` is not `drives * record_len` values.
    DrivesLength,
    /// `responses` is not `controls * record_len` values.
    ResponsesLength,
    /// The SRS is missing for an SRS target, present for a waveform one, or not
    /// `controls * natural frequencies` values.
    SrsShape,
    /// `verdicts` is not one per control.
    VerdictsLength,
    /// A value is not finite.
    NonFinite,
}

impl InputsRule {
    /// The stable name.
    pub fn as_str(self) -> &'static str {
        match self {
            InputsRule::PulseIndex => "pulse_index",
            InputsRule::Level => "level",
            InputsRule::DrivesLength => "drives_length",
            InputsRule::ResponsesLength => "responses_length",
            InputsRule::SrsShape => "srs_shape",
            InputsRule::VerdictsLength => "verdicts_length",
            InputsRule::NonFinite => "non_finite",
        }
    }
}

/// Why a [`PulseInputs`] was refused.
#[derive(Debug, Clone, PartialEq)]
pub struct InputsError {
    pub rule: InputsRule,
    pub message: String,
}

impl fmt::Display for InputsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.rule.as_str())
    }
}

impl std::error::Error for InputsError {}

/// The rule a law's [`PulseOutputs`] broke. The names that the single-output
/// worlds also have are the same (`wrong_length`, `non_finite`,
/// `undeclared_value`, `too_many_values`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputRule {
    /// `drives` is not `drives * record_len` values.
    WrongLength,
    /// A drive value is not finite.
    NonFinite,
    /// `delays_s` is not one finite delay per control.
    DelaysShape,
    /// A value under a key the manifest does not declare, or twice.
    UndeclaredValue,
    /// More than [`MAX_LAW_VALUES`] values, or a value that is not finite.
    TooManyValues,
}

impl OutputRule {
    /// The stable name.
    pub fn as_str(self) -> &'static str {
        match self {
            OutputRule::WrongLength => "wrong_length",
            OutputRule::NonFinite => "non_finite",
            OutputRule::DelaysShape => "delays_shape",
            OutputRule::UndeclaredValue => "undeclared_value",
            OutputRule::TooManyValues => "too_many_values",
        }
    }
}

/// Why a law's [`PulseOutputs`] was refused.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputError {
    pub rule: OutputRule,
    pub message: String,
}

impl fmt::Display for OutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.rule.as_str())
    }
}

impl std::error::Error for OutputError {}

fn finite(xs: &[f64]) -> bool {
    xs.iter().all(|x| x.is_finite())
}

impl PulseInputs {
    /// Check the inputs against the run's `setup`.
    pub fn validate(&self, setup: &Setup) -> Result<(), InputsError> {
        let err = |rule, message: String| Err(InputsError { rule, message });
        let (nc, nd, len) = (setup.n_controls, setup.n_drives, setup.record_len);
        if self.pulse_index == 0 {
            return err(
                InputsRule::PulseIndex,
                "an iteration after no pulse; the first call is the seed".into(),
            );
        }
        if !(self.level_db.is_finite() && self.next_level_db.is_finite()) {
            return err(
                InputsRule::Level,
                format!("levels of {} and {} dB", self.level_db, self.next_level_db),
            );
        }
        if self.drives.len() != nd * len {
            return err(
                InputsRule::DrivesLength,
                format!(
                    "{} drive samples; {} are needed",
                    self.drives.len(),
                    nd * len
                ),
            );
        }
        if self.responses.len() != nc * len {
            return err(
                InputsRule::ResponsesLength,
                format!(
                    "{} response samples; {} are needed",
                    self.responses.len(),
                    nc * len
                ),
            );
        }
        match (&setup.target, &self.srs) {
            (Target::Waveform(_), None) => {}
            (Target::Srs(t), Some(s)) if s.len() == nc * t.natural_hz.len() => {
                if !finite(s) {
                    return err(InputsRule::NonFinite, "a measured SRS value".into());
                }
            }
            (Target::Waveform(_), Some(_)) => {
                return err(
                    InputsRule::SrsShape,
                    "a measured SRS for a waveform target".into(),
                )
            }
            (Target::Srs(_), _) => {
                return err(
                    InputsRule::SrsShape,
                    "the measured SRS is missing or not one per control per frequency".into(),
                )
            }
        }
        if self.verdicts.len() != nc {
            return err(
                InputsRule::VerdictsLength,
                format!("{} verdicts for {nc} controls", self.verdicts.len()),
            );
        }
        if !finite(&self.drives) || !finite(&self.responses) {
            return err(InputsRule::NonFinite, "a drive or response sample".into());
        }
        Ok(())
    }
}

impl PulseOutputs {
    /// Check a law's outputs against the run's `setup` and the value keys its
    /// manifest declares.
    pub fn validate(&self, setup: &Setup, declared: &[String]) -> Result<(), OutputError> {
        let err = |rule, message: String| Err(OutputError { rule, message });
        let want = setup.n_drives * setup.record_len;
        if self.drives.len() != want {
            return err(
                OutputRule::WrongLength,
                format!("{} drive samples; {want} are needed", self.drives.len()),
            );
        }
        if !finite(&self.drives) {
            return err(OutputRule::NonFinite, "a drive sample is not finite".into());
        }
        if let Some(d) = &self.delays_s {
            if d.len() != setup.n_controls || !finite(d) {
                return err(
                    OutputRule::DelaysShape,
                    format!(
                        "{} delays for {} controls; one finite delay per control",
                        d.len(),
                        setup.n_controls
                    ),
                );
            }
        }
        if self.values.len() > MAX_LAW_VALUES || !self.values.iter().all(|v| v.value.is_finite()) {
            return err(
                OutputRule::TooManyValues,
                format!(
                    "{} values; at most {MAX_LAW_VALUES}, each finite",
                    self.values.len()
                ),
            );
        }
        for (i, v) in self.values.iter().enumerate() {
            let twice = self.values[..i].iter().any(|w| w.key == v.key);
            if twice || !declared.iter().any(|k| *k == v.key) {
                return err(
                    OutputRule::UndeclaredValue,
                    format!("the value `{}` is undeclared or given twice", v.key),
                );
            }
        }
        Ok(())
    }
}
