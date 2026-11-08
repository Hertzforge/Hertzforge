//! `servo_blend`: the built-in Sine law.
//!
//! - **Engage.** The starting drive is the identification's prediction at the
//!   start frequency, scaled to the start level (clamped to `1e-4..1e4` drive
//!   units before the level), or 0.05 times the start level's scale where the
//!   prediction is not trusted.
//! - **Each tick.** An amplitude servo (proportional-integral on the error in
//!   dB, its own rate limit) moves its own level from the measured composite
//!   against the target; the blend adds `blend_w` times the change of the
//!   prediction since the last tick, and clamps the total to 1 dB a tick. The
//!   drive is the drive in force moved by that total. While a limit channel
//!   holds the drive (`hold = notch`) or a resonance search maps the plant
//!   (`probe`) the servo is held (it does not wind up); during the search the
//!   blend is not stepped and the drive in force is returned.
//!
//! This is the controller's original arithmetic, moved behind the law seam
//! unchanged: the same servo (`AmplitudeServo::new(1, max_rate)` with
//! `(kp, ki)`, a drive ceiling of 1e6 and a fault fraction of 1e-3), the same
//! blend, the same seed. The law file `laws/sine-servo` is built from this same
//! code.

use crate::control::{BlendConfig, BlendLaw, CompressionPreset};
use crate::law::{
    Hold, LawDescriptor, LawFault, LawValues, LiveParam, ParamSetting, SineControlLaw,
    SineLawFactory, SineLawSetup, TickInputs, TickOutputs, SERVO_BLEND_ID,
};
use hertzforge_dsp::sine::AmplitudeServo;

/// The law's catalog id.
pub const ID: &str = SERVO_BLEND_ID;
/// The law's name in its manifest.
pub const NAME: &str = "Servo blend";
/// The law's version in its manifest.
pub const VERSION: &str = "0.1.0";
/// The law's author in its manifest.
pub const AUTHOR: &str = "Hertzforge contributors";
/// What the law does, for its manifest.
pub const SUMMARY: &str = "Moves the tone's amplitude each tick by a proportional-integral \
    servo on the error in dB, blended with the identification's prediction of the drive at the \
    present frequency. The controller caps every change and holds the drive under the limit \
    channels and the drive ceiling.";

/// The servo's own fault fraction: a measurement below this fraction of the
/// target freezes its level (the controller's servo-fault abort, wider, owns
/// the decision to stop).
pub const SERVO_FAULT_FRACTION: f64 = 1e-3;
/// The servo's ceiling on its own level, drive units (the controller's drive
/// ceiling is the bound that matters).
pub const SERVO_DRIVE_CEILING: f64 = 1e6;
/// The blend's clamp on the whole change in one tick, dB.
pub const BLEND_MAX_STEP_DB: f64 = 1.0;
/// The starting drive without a trusted prediction, times the start level's
/// scale.
pub const UNTRUSTED_SEED: f64 = 0.05;

/// What kind of value a parameter takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KnobKind {
    /// A real number.
    Number {
        unit: &'static str,
        min: f64,
        max: f64,
        step: f64,
        default: f64,
    },
    /// One of a fixed set of named options.
    Choice {
        options: &'static [&'static str],
        default: &'static str,
    },
}

/// One parameter of the law: what a parameter form needs to draw it, and the
/// range the law accepts. The law catalog turns this list into the law's
/// manifest; the law file declares the same manifest from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knob {
    /// The stable key.
    pub key: &'static str,
    /// Name shown on the form.
    pub label: &'static str,
    /// Help text for the form.
    pub help: &'static str,
    pub kind: KnobKind,
    /// Whether the operator may change it while the test runs.
    pub live: bool,
}

/// One value the law reports on each call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiagnosticDecl {
    pub key: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub help: &'static str,
}

/// The compression choices, in form order.
pub const COMPRESSION_OPTIONS: &[&str] = &["slow", "fast", "custom"];

/// The parameters, in form order.
pub const KNOBS: &[Knob] = &[
    Knob {
        key: "blend_w",
        label: "Prediction weight",
        help: "How much of each tick's change follows the identification's prediction of the \
               drive (1) rather than the servo on the measured error (0). It can be changed \
               while the test runs.",
        kind: KnobKind::Number {
            unit: "",
            min: 0.0,
            max: 1.0,
            step: 0.05,
            default: 0.5,
        },
        live: true,
    },
    Knob {
        key: "compression",
        label: "Compression",
        help: "How fast the servo corrects: slow (integral gain 2, at most 20 dB/s), fast (6, \
               at most 60 dB/s), or custom with the two values below.",
        kind: KnobKind::Choice {
            options: COMPRESSION_OPTIONS,
            default: "slow",
        },
        live: false,
    },
    Knob {
        key: "ki",
        label: "Integral gain",
        help: "The servo's integral gain, with custom compression.",
        kind: KnobKind::Number {
            unit: "",
            min: 0.1,
            max: 20.0,
            step: 0.1,
            default: 2.0,
        },
        live: false,
    },
    Knob {
        key: "max_rate_db_per_s",
        label: "Largest servo rate",
        help: "The fastest the servo moves the drive, with custom compression.",
        kind: KnobKind::Number {
            unit: "dB/s",
            min: 1.0,
            max: 240.0,
            step: 1.0,
            default: 20.0,
        },
        live: false,
    },
    Knob {
        key: "kp",
        label: "Proportional gain",
        help: "The servo's proportional gain on the error in dB.",
        kind: KnobKind::Number {
            unit: "",
            min: 0.0,
            max: 2.0,
            step: 0.05,
            default: 0.3,
        },
        live: false,
    },
];

/// The values the law reports on each call, in index order.
pub const DIAGNOSTICS: &[DiagnosticDecl] = &[
    DiagnosticDecl {
        key: "servo_step_db",
        label: "Servo step",
        unit: "dB",
        help: "The servo's part of the last tick's change.",
    },
    DiagnosticDecl {
        key: "prediction_step_db",
        label: "Prediction step",
        unit: "dB",
        help: "The prediction's part of the last tick's change.",
    },
    DiagnosticDecl {
        key: "total_step_db",
        label: "Total step",
        unit: "dB",
        help: "The change the law asked for on the last tick, before the controller's caps.",
    },
    DiagnosticDecl {
        key: "servo_level_db",
        label: "Servo level",
        unit: "dB",
        help: "The servo's own drive level after the last tick, in dB relative to one drive \
               unit.",
    },
];

/// Index of each value in [`DIAGNOSTICS`].
pub const V_SERVO_STEP: usize = 0;
pub const V_PREDICTION_STEP: usize = 1;
pub const V_TOTAL_STEP: usize = 2;
pub const V_SERVO_LEVEL: usize = 3;

/// The values the law is built with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServoBlendParams {
    /// The prediction weight, 0 to 1.
    pub blend_w: f64,
    /// The servo's `(ki, max_rate)` pair.
    pub compression: CompressionPreset,
    /// The servo's proportional gain.
    pub kp: f64,
}

impl Default for ServoBlendParams {
    fn default() -> Self {
        ServoBlendParams {
            blend_w: 0.5,
            compression: CompressionPreset::Slow,
            kp: 0.3,
        }
    }
}

fn number_range(key: &str) -> Option<(f64, f64)> {
    KNOBS
        .iter()
        .find(|k| k.key == key)
        .and_then(|k| match k.kind {
            KnobKind::Number { min, max, .. } => Some((min, max)),
            KnobKind::Choice { .. } => None,
        })
}

impl ServoBlendParams {
    /// Check every value against its declared range.
    pub fn validate(&self) -> Result<(), String> {
        let check = |key: &str, v: f64| -> Result<(), String> {
            let (lo, hi) = number_range(key).unwrap_or((f64::NEG_INFINITY, f64::INFINITY));
            if v.is_finite() && v >= lo && v <= hi {
                Ok(())
            } else {
                Err(format!("{key} is {v}; it runs from {lo} to {hi}"))
            }
        };
        check("blend_w", self.blend_w)?;
        check("kp", self.kp)?;
        if let CompressionPreset::Custom {
            ki,
            max_rate_db_per_s,
        } = self.compression
        {
            check("ki", ki)?;
            check("max_rate_db_per_s", max_rate_db_per_s)?;
        }
        Ok(())
    }

    /// The parameter values as the run's records hold them, in [`KNOBS`] order.
    pub fn settings(&self) -> Vec<(String, ParamSetting)> {
        let (name, (ki, rate)) = match self.compression {
            CompressionPreset::Slow => ("slow", CompressionPreset::Slow.gains()),
            CompressionPreset::Fast => ("fast", CompressionPreset::Fast.gains()),
            c @ CompressionPreset::Custom { .. } => ("custom", c.gains()),
        };
        vec![
            ("blend_w".into(), ParamSetting::Number(self.blend_w)),
            ("compression".into(), ParamSetting::Choice(name.into())),
            ("ki".into(), ParamSetting::Number(ki)),
            ("max_rate_db_per_s".into(), ParamSetting::Number(rate)),
            ("kp".into(), ParamSetting::Number(self.kp)),
        ]
    }

    /// Read the values from the records' form (as a law file's parameters
    /// arrive). A key that is absent takes its default; an unknown key, a
    /// value of the wrong kind or out of range is refused, by name.
    pub fn from_settings(settings: &[(String, ParamSetting)]) -> Result<Self, String> {
        let mut p = ServoBlendParams::default();
        let mut compression = "slow".to_string();
        let (mut ki, mut rate) = (2.0, 20.0);
        for (key, v) in settings {
            let number = |v: &ParamSetting| -> Result<f64, String> {
                match v {
                    ParamSetting::Number(x) => Ok(*x),
                    ParamSetting::Integer(i) => Ok(*i as f64),
                    _ => Err(format!("{key} takes a number")),
                }
            };
            match key.as_str() {
                "blend_w" => p.blend_w = number(v)?,
                "kp" => p.kp = number(v)?,
                "ki" => ki = number(v)?,
                "max_rate_db_per_s" => rate = number(v)?,
                "compression" => match v {
                    ParamSetting::Choice(c) if COMPRESSION_OPTIONS.contains(&c.as_str()) => {
                        compression = c.clone()
                    }
                    _ => {
                        return Err(format!(
                            "compression is one of {}",
                            COMPRESSION_OPTIONS.join(", ")
                        ))
                    }
                },
                other => return Err(format!("{other} is not a parameter of this law")),
            }
        }
        p.compression = match compression.as_str() {
            "fast" => CompressionPreset::Fast,
            "custom" => CompressionPreset::Custom {
                ki,
                max_rate_db_per_s: rate,
            },
            _ => CompressionPreset::Slow,
        };
        p.validate()?;
        Ok(p)
    }

    /// The law's descriptor for these values.
    pub fn descriptor(&self) -> LawDescriptor {
        LawDescriptor {
            id: ID.into(),
            name: NAME.into(),
            version: VERSION.into(),
            sha256: None,
            params: self.settings(),
            diagnostics: DIAGNOSTICS.iter().map(|d| d.key.to_string()).collect(),
            live: live_params(),
        }
    }
}

/// The live parameters with their ranges.
pub fn live_params() -> Vec<LiveParam> {
    KNOBS
        .iter()
        .filter(|k| k.live)
        .filter_map(|k| match k.kind {
            KnobKind::Number { min, max, .. } => Some(LiveParam {
                key: k.key.into(),
                min,
                max,
            }),
            KnobKind::Choice { .. } => None,
        })
        .collect()
}

/// The built-in law: today's amplitude servo and feedforward blend.
///
/// The law follows the drive the core played. Each tick it compares the drive
/// in force with the drive it asked for on the tick before; where the core
/// held the drive under the request (the notch ceiling clamping it, a cap, the
/// drive ceiling, the first-drive bound), the servo's own level is moved down
/// by the same amount. The servo's steps are relative, so this changes no step
/// it takes; it keeps the servo's level with the drive, so a long clamp cannot
/// wind it up to its ceiling and leave it unable to raise the drive when the
/// clamp ends.
#[derive(Debug, Clone)]
pub struct ServoBlendLaw {
    servo: AmplitudeServo,
    blend: BlendLaw,
    /// The drive the law asked for on its last call (0 before the engage).
    asked: f64,
}

impl ServoBlendLaw {
    /// The law for `params`. Refuses only what the servo and the blend cannot
    /// run with (a rate that is not above zero, gains that are not numbers, a
    /// weight outside 0 to 1); the declared ranges are the catalog's check
    /// ([`ServoBlendParams::validate`]).
    pub fn new(params: &ServoBlendParams) -> Result<Self, LawFault> {
        let (ki, max_rate) = params.compression.gains();
        if !(max_rate.is_finite() && max_rate > 0.0 && ki.is_finite() && params.kp.is_finite()) {
            return Err(LawFault::Failed(format!(
                "the servo needs a rate above zero and finite gains (rate {max_rate}, ki {ki}, \
                 kp {})",
                params.kp
            )));
        }
        let servo = AmplitudeServo::new(1.0, max_rate)
            .with_gains(params.kp, ki)
            .with_drive_ceiling(SERVO_DRIVE_CEILING)
            .with_fault_fraction(SERVO_FAULT_FRACTION);
        let blend = BlendLaw::new(BlendConfig {
            w: params.blend_w,
            max_step_db_per_tick: BLEND_MAX_STEP_DB,
        })
        .map_err(|e| LawFault::Failed(e.to_string()))?;
        Ok(ServoBlendLaw::from_parts(servo, blend))
    }

    /// The law from a servo and a blend already built (a step built without a
    /// law of its own takes these).
    pub fn from_parts(servo: AmplitudeServo, blend: BlendLaw) -> Self {
        ServoBlendLaw {
            servo,
            blend,
            asked: 0.0,
        }
    }

    /// Move the servo's level down by what the core held the drive under the
    /// last request. The servo's level does not set the drive here (the drive
    /// moves by the servo's step from the drive in force), so the reseat moves
    /// no drive; it is what keeps the level with the drive.
    fn follow_the_drive_played(&mut self, played: f64) {
        if self.asked > 0.0 && played > 0.0 && played < self.asked * (1.0 - HELD_UNDER) {
            let level = self.servo.drive_scale() * (played / self.asked);
            self.servo.set_feedforward_scale(level);
        }
    }
}

/// How far under its request the drive played must be before the law follows
/// it down, as a fraction: well above rounding, well under any cap's step.
const HELD_UNDER: f64 = 1e-9;

/// Write the values; a law's own values are always declared, so a refusal
/// here is the law's bug and is reported as one.
fn put(values: &mut LawValues, servo: f64, pred: f64, total: f64, level: f64) {
    for (i, v) in [
        (V_SERVO_STEP, servo),
        (V_PREDICTION_STEP, pred),
        (V_TOTAL_STEP, total),
        (V_SERVO_LEVEL, level),
    ] {
        let _ = values.set(i, v);
    }
}

impl SineControlLaw for ServoBlendLaw {
    fn name(&self) -> &str {
        NAME
    }

    fn engage(
        &mut self,
        inputs: &TickInputs<'_>,
        values: &mut LawValues,
    ) -> Result<TickOutputs, LawFault> {
        let seed = match inputs.prediction_db {
            Some(db) => 10f64.powf(db / 20.0).clamp(1e-4, 1e4),
            None => UNTRUSTED_SEED,
        };
        let drive = 10f64.powf(inputs.level_db / 20.0) * seed;
        self.asked = drive;
        put(
            values,
            0.0,
            0.0,
            0.0,
            20.0 * self.servo.drive_scale().log10(),
        );
        Ok(TickOutputs {
            drive,
            faulted: false,
        })
    }

    fn tick(
        &mut self,
        inputs: &TickInputs<'_>,
        values: &mut LawValues,
    ) -> Result<TickOutputs, LawFault> {
        self.follow_the_drive_played(inputs.drive);
        let feed = inputs.measured / inputs.target.max(1e-12);
        let before = self.servo.drive_scale();
        let after = match inputs.hold {
            Hold::Free => self.servo.update(feed, inputs.dt_s),
            Hold::Notch | Hold::Probe => self.servo.hold_tick(feed),
        };
        let servo_step_db = if before > 0.0 && after > 0.0 {
            20.0 * (after / before).log10()
        } else {
            0.0
        };
        let level = 20.0 * self.servo.drive_scale().log10();
        if inputs.hold == Hold::Probe {
            // The search maps the plant at a fixed drive: the blend does not
            // move, and the drive in force is what the law asks for.
            put(values, servo_step_db, 0.0, 0.0, level);
            self.asked = inputs.drive;
            return Ok(TickOutputs {
                drive: inputs.drive,
                faulted: self.servo.faulted(),
            });
        }
        let (delta_db, prediction_db) = self.blend.step_parts(inputs.prediction_db, servo_step_db);
        put(values, servo_step_db, prediction_db, delta_db, level);
        let drive = inputs.drive * 10f64.powf(delta_db / 20.0);
        self.asked = drive;
        Ok(TickOutputs {
            drive,
            faulted: self.servo.faulted(),
        })
    }

    fn adjust(&mut self, key: &str, value: f64) -> Result<(), LawFault> {
        match key {
            "blend_w" => self
                .blend
                .set_w(value)
                .map_err(|e| LawFault::Failed(e.to_string())),
            other => Err(LawFault::Failed(format!(
                "{other} cannot be changed while the test runs"
            ))),
        }
    }
}

/// Builds [`ServoBlendLaw`] for a run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServoBlendFactory {
    pub params: ServoBlendParams,
}

impl ServoBlendFactory {
    /// A factory for `params`, checked.
    pub fn new(params: ServoBlendParams) -> Result<Self, String> {
        params.validate()?;
        Ok(ServoBlendFactory { params })
    }
}

impl SineLawFactory for ServoBlendFactory {
    fn build(&self, _setup: &SineLawSetup<'_>) -> Result<Box<dyn SineControlLaw + Send>, LawFault> {
        Ok(Box::new(ServoBlendLaw::new(&self.params)?))
    }

    fn describe(&self) -> LawDescriptor {
        self.params.descriptor()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_round_trip_and_bad_values_are_refused_by_name() {
        let p = ServoBlendParams {
            blend_w: 0.25,
            compression: CompressionPreset::Custom {
                ki: 4.0,
                max_rate_db_per_s: 40.0,
            },
            kp: 0.5,
        };
        assert_eq!(ServoBlendParams::from_settings(&p.settings()).unwrap(), p);
        assert_eq!(
            ServoBlendParams::from_settings(&[]).unwrap(),
            ServoBlendParams::default()
        );
        for (key, v, why) in [
            ("blend_w", ParamSetting::Number(1.5), "blend_w is 1.5"),
            ("kp", ParamSetting::Toggle(true), "kp takes a number"),
            ("nope", ParamSetting::Number(1.0), "nope is not a parameter"),
            (
                "compression",
                ParamSetting::Choice("medium".into()),
                "compression is one of",
            ),
        ] {
            let e = ServoBlendParams::from_settings(&[(key.into(), v)]).unwrap_err();
            assert!(e.starts_with(why), "{e}");
        }
        let d = ServoBlendParams::default().descriptor();
        assert_eq!(d.id, "servo_blend");
        assert_eq!(d.diagnostics.len(), DIAGNOSTICS.len());
        assert_eq!(
            d.live,
            vec![LiveParam {
                key: "blend_w".into(),
                min: 0.0,
                max: 1.0
            }]
        );
    }

    #[test]
    fn only_the_prediction_weight_changes_live() {
        let mut law = ServoBlendLaw::new(&ServoBlendParams::default()).unwrap();
        assert!(law.adjust("blend_w", 0.2).is_ok());
        assert!(law.adjust("blend_w", 2.0).is_err());
        assert!(law.adjust("kp", 0.5).is_err());
    }
}
