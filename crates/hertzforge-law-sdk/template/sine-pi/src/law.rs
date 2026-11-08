//! `sine-pi`: a proportional-integral loop on the error in dB.
//!
//! - **Engage.** The starting drive is the identification's prediction at the
//!   start frequency, at the start level, moved by `start_offset_db`; without a
//!   trusted prediction, 0.05 drive units at the start level (moved the same
//!   way).
//! - **Until the measurement comes up.** The starting drive is held for at
//!   least [`WAIT_TICKS`] ticks, and then until the measured amplitude is
//!   within [`COME_UP_DB`] of what the identification predicts for that drive
//!   (or over it), for at most [`HOLD_MAX_S`]; without a trusted prediction,
//!   for [`HOLD_MAX_S`]. A drive's response reaches the measurement late: on
//!   hardware the converters and the output buffer delay it, and the
//!   controller's tracking filter, whose corner scales with the frequency,
//!   takes a third of a second to read it at 20 Hz. On the way up the reading
//!   can stall and dip, so a rise that has stopped is no sign that it has come
//!   up. An error read before then is one the law did not cause, and a law
//!   that acts on it climbs blind.
//! - **Each tick.** The error is `20 log10(target / measured)`. The drive in
//!   force moves by `kp` times the error's change since the last tick, plus
//!   `ki` times the error times the tick, plus `follow` times the prediction's
//!   change since the last tick, at most `max_step_db` either way. While the
//!   controller holds the drive (`notch` or `probe`) the law holds it too. A
//!   change of level is not held: the measurement already reads the drive in
//!   force, so the error is the step itself, taken at most `max_step_db` a
//!   tick.
//!
//! `ki` can be changed while the test runs.

use hertzforge_law_sdk::sine::*;
use hertzforge_law_sdk::{
    NumberSpec, Param, ParamKind, ParamSpec, ParamValue, Params, SineSingleLaw,
};

/// Ticks after engage the drive is held at least (the controller's tracking
/// filters take a few ticks to read the new tone).
pub const WAIT_TICKS: u64 = 5;

/// How far under what the identification predicts for the starting drive the
/// measurement may read and count as come up, dB.
pub const COME_UP_DB: f64 = 2.0;

/// The longest the starting drive is held, seconds, whatever the measurement
/// does.
pub const HOLD_MAX_S: f64 = 1.0;

/// The law's state.
pub struct SinePi {
    kp: f64,
    ki: f64,
    follow: f64,
    max_step_db: f64,
    start_offset_db: f64,
    prev_error_db: Option<f64>,
    prev_prediction_db: Option<f64>,
    /// Whether the starting drive is still held, and for how long it has been.
    waiting: bool,
    waited_s: f64,
}

/// A number parameter: `(min, max, step, default)` in `unit`.
fn number(key: &str, label: &str, help: &str, unit: &str, range: [f64; 4]) -> ParamSpec {
    let [min, max, step, default] = range;
    ParamSpec {
        key: key.into(),
        label: label.into(),
        help: help.into(),
        kind: ParamKind::Number(NumberSpec {
            unit: unit.into(),
            min,
            max,
            step,
            default,
        }),
    }
}

impl SineSingleLaw for SinePi {
    fn describe() -> SineManifest {
        SineManifestBuilder::new("Sine PI", "0.1.0")
            .author("Hertzforge contributors")
            .summary(
                "Moves the tone's amplitude each tick by a proportional-integral loop on the \
                 error in dB, following the identification's prediction from tick to tick.",
            )
            .param(number(
                "kp",
                "Proportional gain",
                "How much of the error's change since the last tick moves the drive.",
                "",
                [0.0, 2.0, 0.05, 0.3],
            ))
            .param(number(
                "ki",
                "Integral gain",
                "How fast the drive closes the error, per second. It can be changed while the \
                 test runs.",
                "1/s",
                [0.0, 20.0, 0.5, 4.0],
            ))
            .param(number(
                "follow",
                "Prediction weight",
                "How much of the prediction's change from tick to tick the drive follows.",
                "",
                [0.0, 1.0, 0.05, 1.0],
            ))
            .param(number(
                "max_step_db",
                "Largest step per tick",
                "The most the law moves the drive in one tick.",
                "dB",
                [0.1, 1.0, 0.1, 1.0],
            ))
            .param(number(
                "start_offset_db",
                "Start offset",
                "Moves the law's own first drive, for seeing how the controller's guards \
                 respond.",
                "dB",
                [-20.0, 20.0, 0.5, 0.0],
            ))
            .value(
                "error_db",
                "Error",
                "dB",
                "The target over the measured amplitude, in dB.",
            )
            .value(
                "step_db",
                "Step",
                "dB",
                "How far the law moved the drive this tick.",
            )
            .live("ki")
            .build()
    }

    fn new(_setup: SineSetup, params: Params) -> Result<Self, String> {
        Ok(SinePi {
            kp: params.number("kp")?,
            ki: params.number("ki")?,
            follow: params.number("follow")?,
            max_step_db: params.number("max_step_db")?,
            start_offset_db: params.number("start_offset_db")?,
            prev_error_db: None,
            prev_prediction_db: None,
            waiting: true,
            waited_s: 0.0,
        })
    }

    fn engage(&mut self, i: &TickInputs) -> Result<TickOutputs, String> {
        let offset = i.level_db + self.start_offset_db;
        let drive = match i.prediction_db {
            Some(db) => 10f64.powf((db + offset) / 20.0),
            None => 0.05 * 10f64.powf(offset / 20.0),
        };
        self.prev_prediction_db = i.prediction_db;
        Ok(tick_outputs(drive, &[("error_db", 0.0), ("step_db", 0.0)]))
    }

    fn tick(&mut self, i: &TickInputs) -> Result<TickOutputs, String> {
        let error_db = if i.measured > 0.0 && i.target > 0.0 {
            20.0 * (i.target / i.measured).log10()
        } else {
            0.0
        };
        let prediction_step = match (self.prev_prediction_db, i.prediction_db) {
            (Some(prev), Some(now)) => self.follow * (now - prev),
            _ => 0.0,
        };
        self.prev_prediction_db = i.prediction_db;
        if self.waiting {
            self.waited_s += i.dt_s;
            // The prediction is the drive that meets the target at this level,
            // so the drive in force should read the target scaled by their
            // ratio.
            let expected = i
                .prediction_db
                .map(|db| i.target * i.drive / 10f64.powf((db + i.level_db) / 20.0));
            let up = match expected {
                Some(e) if e > 0.0 && i.measured > 0.0 => {
                    20.0 * (i.measured / e).log10() > -COME_UP_DB
                }
                _ => false,
            };
            self.waiting = i.tick_index <= WAIT_TICKS || (!up && self.waited_s < HOLD_MAX_S);
        }
        let holding = self.waiting || i.hold != Hold::Free;
        let step_db = if holding {
            0.0
        } else {
            let change = self.prev_error_db.map_or(0.0, |prev| error_db - prev);
            (self.kp * change + self.ki * error_db * i.dt_s + prediction_step)
                .clamp(-self.max_step_db, self.max_step_db)
        };
        self.prev_error_db = Some(error_db);
        let drive = i.drive * 10f64.powf(step_db / 20.0);
        Ok(tick_outputs(
            drive,
            &[("error_db", error_db), ("step_db", step_db)],
        ))
    }

    fn adjust(&mut self, p: &Param) -> Result<(), String> {
        match (p.key.as_str(), &p.value) {
            ("ki", ParamValue::Number(v)) => {
                self.ki = *v;
                Ok(())
            }
            (key, _) => Err(format!("`{key}` cannot be changed while the test runs")),
        }
    }
}
