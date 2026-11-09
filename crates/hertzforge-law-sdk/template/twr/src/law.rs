//! The frequency-domain correction, written plainly.
//!
//! - The first drive is the plant's inverse applied to the reference:
//!   `D = G R` on the bins the controller trusts.
//! - After each correction play the drive spectrum moves by a fraction `beta`
//!   of the inverse times the error: `D += beta G (R - Y)`.
//! - When the controller clipped the last drive, the correction starts from
//!   the drive that played rather than the one this law asked for.
//!
//! Two more parameters are there for seeing how the controller's guards
//! respond. `correct` off holds the first drive for the whole run, so a play
//! that starts wrong stays wrong and the controller ends the run when it does
//! not converge. `model_error_db` scales the law's own copy of the plant's
//! inverse, as an identification that was that many dB wrong would: -6 dB
//! drives half as hard (every play half the target), -4 dB leaves 0.685 of the
//! error after each correction at `beta` 0.5 instead of 0.5, and +8 dB starts
//! 2.5 times hot (the controller's first-drive bound).

use hertzforge_law_sdk::twr::*;
use hertzforge_law_sdk::{
    DiagnosticSpec, Manifest, NumberSpec, ParamKind, ParamSpec, Params, TwrSingleLaw,
};

/// The law's state for one run.
pub struct Correction {
    beta: f64,
    correct: bool,
    /// The reference's spectrum.
    r: Vec<Cplx>,
    /// The inverse, as this law believes it.
    g: Vec<Cplx>,
    trusted: Vec<bool>,
    /// The drive spectrum.
    d: Vec<Cplx>,
    n: usize,
}

fn number(
    key: &str,
    label: &str,
    unit: &str,
    help: &str,
    min: f64,
    max: f64,
    default: f64,
) -> ParamSpec {
    ParamSpec {
        key: key.into(),
        label: label.into(),
        help: help.into(),
        kind: ParamKind::Number(NumberSpec {
            unit: unit.into(),
            min,
            max,
            step: 0.05,
            default,
        }),
    }
}

impl Correction {
    fn drive(&self) -> Result<PlayOutputs, String> {
        let drive = irfft_half(&self.d, self.n)?;
        let peak = drive.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        Ok(outputs(drive, &[("drive_peak_v", peak)]))
    }
}

impl TwrSingleLaw for Correction {
    fn describe() -> Manifest {
        Manifest {
            name: "plain correction".into(),
            version: "0.1.0".into(),
            author: String::new(),
            summary: "The plant's inverse applied to the reference, then a fraction of the \
                      inverse times the error after each correction play."
                .into(),
            params: vec![
                number(
                    "beta",
                    "Correction gain",
                    "",
                    "The fraction of each play's error corrected before the next play.",
                    0.05,
                    1.0,
                    0.5,
                ),
                ParamSpec {
                    key: "correct".into(),
                    label: "Correct after each play".into(),
                    help: "Off: the first drive is held for the whole run.".into(),
                    kind: ParamKind::Toggle(true),
                },
                number(
                    "model_error_db",
                    "Model error",
                    "dB",
                    "Scales this law's copy of the plant's inverse, for seeing how the \
                     controller's guards respond.",
                    -12.0,
                    12.0,
                    0.0,
                ),
            ],
            diagnostics: vec![DiagnosticSpec {
                key: "drive_peak_v".into(),
                label: "Drive peak".into(),
                unit: "V".into(),
                help: "The full-level drive's peak.".into(),
            }],
        }
    }

    fn new(setup: TwrSetup, params: Params) -> Result<Self, String> {
        let k = 10f64.powf(params.number("model_error_db")? / 20.0);
        let g = inverse(&setup.plant)
            .into_iter()
            .map(|(re, im)| (re * k, im * k))
            .collect();
        Ok(Correction {
            beta: params.number("beta")?,
            correct: params.toggle("correct")?,
            r: rfft_half(&setup.reference)?,
            g,
            trusted: setup.plant.trusted.clone(),
            d: Vec::new(),
            n: setup.record_len as usize,
        })
    }

    fn seed(&mut self, _inputs: &PlayInputs) -> Result<PlayOutputs, String> {
        self.d = self
            .r
            .iter()
            .zip(&self.g)
            .zip(&self.trusted)
            .map(|((&r, &g), &t)| if t { mul(g, r) } else { (0.0, 0.0) })
            .collect();
        self.drive()
    }

    fn iterate(&mut self, inputs: &PlayInputs) -> Result<PlayOutputs, String> {
        if self.correct {
            if inputs.limited {
                // The controller clipped the last drive: correct what played.
                self.d = rfft_half(&inputs.drive)?;
            }
            let y = rfft_half(&inputs.response)?;
            // Bin by bin: the reference, the inverse, the trusted flags and the
            // response all move together, and the index says so plainly.
            #[allow(clippy::needless_range_loop)]
            for k in 0..self.d.len() {
                if self.trusted[k] {
                    let e = (self.r[k].0 - y[k].0, self.r[k].1 - y[k].1);
                    let step = mul(self.g[k], e);
                    self.d[k].0 += self.beta * step.0;
                    self.d[k].1 += self.beta * step.1;
                }
            }
        }
        self.drive()
    }
}
