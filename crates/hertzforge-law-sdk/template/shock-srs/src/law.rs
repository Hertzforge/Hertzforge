//! The per-band ratio update, written plainly.
//!
//! - The starting gain of each component is one over the identified gain at
//!   its frequency, so the first pulse's response lands near the target.
//! - After each pulse every component is scaled by the target over the
//!   measured response at the band nearest it, limited to a largest change
//!   per pulse either way. A band with no response, or no target, holds.
//!
//! Two more parameters are there for seeing how the controller's guards
//! respond. `correct` off holds the starting gains for the whole run, so a
//! pulse that starts wrong stays wrong and the controller ends the run when it
//! does not converge. `seed_error_db` scales the starting gains, as an
//! identification that many dB wrong would: -4 dB starts every band 4 dB low.

use hertzforge_law_sdk::srs::*;
use hertzforge_law_sdk::{
    DiagnosticSpec, Manifest, NumberSpec, ParamKind, ParamSpec, Params, ShockSrsLaw,
};

/// The law's state for one run.
pub struct PlainRatio {
    max_change: f64,
    correct: bool,
    /// For each component, the band nearest its frequency.
    nearest: Vec<usize>,
    /// The starting gains.
    start: Vec<f64>,
}

fn number(
    key: &str,
    label: &str,
    unit: &str,
    help: &str,
    (min, max, step, default): (f64, f64, f64, f64),
) -> ParamSpec {
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

impl ShockSrsLaw for PlainRatio {
    fn describe() -> Manifest {
        Manifest {
            name: "plain ratio".into(),
            version: "0.1.0".into(),
            author: String::new(),
            summary: "Starts each component from the inverse of the identified gain, then \
                      scales it by the target over the measured response at its band after \
                      each pulse."
                .into(),
            params: vec![
                number(
                    "max_db_per_pulse",
                    "Largest change per pulse",
                    "dB",
                    "The most any one component may rise or fall from one pulse to the next.",
                    (0.5, 5.0, 0.5, 3.0),
                ),
                ParamSpec {
                    key: "correct".into(),
                    label: "Correct after each pulse".into(),
                    help: "Off: the starting gains are held for the whole run.".into(),
                    kind: ParamKind::Toggle(true),
                },
                number(
                    "seed_error_db",
                    "Starting error",
                    "dB",
                    "Scales this law's starting gains, for seeing how the controller's \
                     guards respond.",
                    (-12.0, 12.0, 0.5, 0.0),
                ),
            ],
            diagnostics: vec![DiagnosticSpec {
                key: "worst_band_error_db".into(),
                label: "Worst band error".into(),
                unit: "dB".into(),
                help: "The last pulse's band furthest from its target (0 at the start).".into(),
            }],
        }
    }

    fn new(setup: SrsSetup, params: Params) -> Result<Self, String> {
        let k = 10f64.powf(params.number("seed_error_db")? / 20.0);
        let start = match &setup.component_gain {
            Some(h) => h.iter().map(|g| k / g).collect(),
            None => vec![k; setup.component_hz.len()],
        };
        Ok(PlainRatio {
            max_change: 10f64.powf(params.number("max_db_per_pulse")? / 20.0),
            correct: params.toggle("correct")?,
            nearest: setup
                .component_hz
                .iter()
                .map(|&f| nearest_band(&setup.band_hz, f))
                .collect(),
            start,
        })
    }

    fn seed(&mut self, _inputs: &SrsInputs) -> Result<SrsOutputs, String> {
        Ok(srs_outputs(
            self.start.clone(),
            &[("worst_band_error_db", 0.0)],
        ))
    }

    fn iterate(&mut self, inputs: &SrsInputs) -> Result<SrsOutputs, String> {
        let mut worst = 0.0f64;
        for (t, m) in inputs.target.iter().zip(&inputs.measured) {
            if *t > 0.0 && *m > 0.0 {
                let db = 20.0 * (m / t).log10();
                if db.abs() > worst.abs() {
                    worst = db;
                }
            }
        }
        let gains = self
            .nearest
            .iter()
            .map(|&b| {
                let (t, m) = (inputs.target[b], inputs.measured[b]);
                if !self.correct || t <= 0.0 || m <= 0.0 {
                    1.0
                } else {
                    (t / m).clamp(1.0 / self.max_change, self.max_change)
                }
            })
            .collect();
        Ok(srs_outputs(gains, &[("worst_band_error_db", worst)]))
    }
}
