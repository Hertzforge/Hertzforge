//! Per-line ratio correction: a single-output Random control law.
//!
//! - **Seed.** The starting drive is the full-level target divided by the
//!   plant power gain, line by line: `drive[k] = target[k] / max(|H|^2[k],
//!   floor)`, with the floor at a thousandth of the in-band peak `|H|^2` so a
//!   deep anti-resonance cannot ask for an unbounded drive. Lines outside the
//!   band are zero.
//! - **Each loop.** Every in-band line whose coherence is at or above the
//!   floor moves its drive by `gain * 10 * log10(target_at_level /
//!   measured)` dB, limited to `+/- max_step_db`. Lines below the coherence
//!   floor hold their drive, and so do lines the limiter is holding down.
//! - **Level.** There is no separate level loop: each line is corrected
//!   toward the target at the scheduled level, so the level follows. A level
//!   change scales the whole drive at once by the power ratio of the new
//!   level to the old. The reported level command is the in-band drive power
//!   relative to the full-level seed, in dB.
//!
//! The controller applies its own protections to the drive this law returns.

use hertzforge_law_sdk::{
    band_range, export_law, inband_rms, DiagnosticSpec, DiagnosticValue, LoopDiagnostics,
    LoopInputs, LoopOutputs, Manifest, NumberSpec, ParamKind, ParamSpec, Params, RandomLaw, Setup,
};

/// Smallest value a spectral density is taken to have when forming a ratio.
const TINY: f64 = 1e-30;
/// The seed's plant-gain floor, as a fraction of the in-band peak `|H|^2`.
const H_FLOOR_FRACTION: f64 = 1e-3;

/// The law's state for one run.
pub struct RatioLaw {
    target: Vec<f64>,
    lo: usize,
    hi: usize,
    full_rms: f64,
    gain: f64,
    max_step_db: f64,
    coherence_floor: f64,
    /// The full-level seed drive: the reference the level command is read
    /// against.
    reference: Vec<f64>,
    drive: Vec<f64>,
    /// The scheduled level as an amplitude ratio to the full target.
    level_scale: f64,
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

fn lines(key: &str, label: &str, help: &str) -> DiagnosticSpec {
    DiagnosticSpec {
        key: key.into(),
        label: label.into(),
        unit: "lines".into(),
        help: help.into(),
    }
}

impl RatioLaw {
    fn check(&self, inputs: &LoopInputs) -> Result<(), String> {
        let n = self.target.len();
        if inputs.measured_psd.len() != n
            || inputs.h_mag2.len() != n
            || inputs.coherence.len() != n
            || (!inputs.notched_bins.is_empty() && inputs.notched_bins.len() != n)
        {
            return Err(format!("the loop inputs are not on the {n}-line grid"));
        }
        Ok(())
    }

    fn level_power(&self) -> f64 {
        self.level_scale * self.level_scale
    }

    fn level_cmd_db(&self) -> f64 {
        let reference: f64 = self.reference[self.lo..self.hi].iter().sum();
        let drive: f64 = self.drive[self.lo..self.hi].iter().sum();
        if reference > 0.0 && drive > 0.0 {
            10.0 * (drive / reference).log10()
        } else {
            20.0 * self.level_scale.log10()
        }
    }

    /// The RMS over the band of `10 * log10(measured / target_at_level)`.
    fn inband_err_db(&self, inputs: &LoopInputs) -> f64 {
        let p = self.level_power();
        let mut sq = 0.0;
        for k in self.lo..self.hi {
            let e =
                10.0 * (inputs.measured_psd[k].max(TINY) / (self.target[k] * p).max(TINY)).log10();
            sq += e * e;
        }
        (sq / (self.hi - self.lo) as f64).sqrt()
    }

    fn outputs(
        &self,
        untrusted: Vec<u32>,
        clamped: Vec<u32>,
        diagnostics_of: Option<&LoopInputs>,
    ) -> LoopOutputs {
        let values = vec![
            DiagnosticValue {
                key: "untrusted_lines".into(),
                value: untrusted.len() as f64,
            },
            DiagnosticValue {
                key: "clamped_lines".into(),
                value: clamped.len() as f64,
            },
        ];
        let (inband_err_db, inband_rms_g) = match diagnostics_of {
            Some(i) => (
                Some(self.inband_err_db(i)),
                Some(inband_rms(&i.measured_psd, i.band, i.df_hz)),
            ),
            None => (None, None),
        };
        LoopOutputs {
            drive_psd: self.drive.clone(),
            level_cmd_db: self.level_cmd_db(),
            diagnostics: LoopDiagnostics {
                untrusted_bins: untrusted,
                clamped_bins: clamped,
                divergent_bins: Vec::new(),
                inband_err_db,
                inband_rms_g,
                servo_faulted: false,
                values,
            },
        }
    }
}

impl RandomLaw for RatioLaw {
    fn describe() -> Manifest {
        Manifest {
            name: "per-line ratio".into(),
            version: "0.1.0".into(),
            author: "Hertzforge contributors".into(),
            summary: "Moves each trusted in-band line's drive toward the target by a fraction of \
                      the measured error in dB, limited per loop. Lines below the coherence \
                      floor hold their drive. The level follows from the per-line correction; a \
                      level change scales the whole drive at once."
                .into(),
            params: vec![
                number(
                    "gain",
                    "Correction gain",
                    "",
                    "The fraction of each line's measured error, in dB, corrected per loop. 1 \
                     corrects the whole error at once; smaller values average out estimation \
                     noise over several loops.",
                    (0.0, 1.0, 0.05, 0.5),
                ),
                number(
                    "max_step_db",
                    "Largest step per loop",
                    "dB",
                    "The most any one line's drive may move in one loop.",
                    (0.1, 12.0, 0.1, 2.0),
                ),
                number(
                    "coherence_floor",
                    "Coherence floor",
                    "",
                    "Lines whose coherence is below this hold their drive instead of chasing a \
                     measurement that cannot be trusted.",
                    (0.0, 1.0, 0.05, 0.5),
                ),
            ],
            diagnostics: vec![
                lines(
                    "untrusted_lines",
                    "Untrusted lines",
                    "In-band lines held this loop because their coherence was below the floor.",
                ),
                lines(
                    "clamped_lines",
                    "Clamped lines",
                    "In-band lines whose correction was limited to the largest step this loop.",
                ),
            ],
        }
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        let n = setup.target_psd.len();
        let r = band_range(setup.band);
        if r.start >= r.end || r.end > n {
            return Err(format!(
                "band [{}, {}) does not fit {n} lines",
                r.start, r.end
            ));
        }
        let full_rms = inband_rms(&setup.target_psd, setup.band, setup.df_hz);
        if full_rms <= 0.0 || full_rms.is_nan() {
            return Err("the target has no in-band energy".into());
        }
        Ok(RatioLaw {
            target: setup.target_psd,
            lo: r.start,
            hi: r.end,
            full_rms,
            gain: params.number("gain")?,
            max_step_db: params.number("max_step_db")?,
            coherence_floor: params.number("coherence_floor")?,
            reference: vec![0.0; n],
            drive: vec![0.0; n],
            level_scale: 1.0,
        })
    }

    fn seed(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, String> {
        self.check(inputs)?;
        let peak = inputs.h_mag2[self.lo..self.hi]
            .iter()
            .copied()
            .fold(0.0, f64::max);
        if peak <= 0.0 || peak.is_nan() {
            return Err("the plant estimate is zero across the band".into());
        }
        let floor = peak * H_FLOOR_FRACTION;
        let p = self.level_power();
        let mut untrusted = Vec::new();
        for k in self.lo..self.hi {
            self.reference[k] = self.target[k] / inputs.h_mag2[k].max(floor);
            self.drive[k] = self.reference[k] * p;
            if inputs.coherence[k] < self.coherence_floor {
                untrusted.push(k as u32);
            }
        }
        Ok(self.outputs(untrusted, Vec::new(), None))
    }

    fn iterate(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, String> {
        self.check(inputs)?;
        let p = self.level_power();
        let mut untrusted = Vec::new();
        let mut clamped = Vec::new();
        for k in self.lo..self.hi {
            if inputs.coherence[k] < self.coherence_floor {
                untrusted.push(k as u32);
                continue;
            }
            if inputs.notched_bins.get(k).copied().unwrap_or(false) {
                continue;
            }
            let target = (self.target[k] * p).max(TINY);
            let measured = inputs.measured_psd[k].max(TINY);
            let mut step_db = self.gain * 10.0 * (target / measured).log10();
            if step_db > self.max_step_db {
                step_db = self.max_step_db;
                clamped.push(k as u32);
            } else if step_db < -self.max_step_db {
                step_db = -self.max_step_db;
                clamped.push(k as u32);
            }
            self.drive[k] *= 10f64.powf(step_db / 10.0);
        }
        Ok(self.outputs(untrusted, clamped, Some(inputs)))
    }

    fn retarget_level(&mut self, target_rms: f64, feedforward_scale: f64) {
        let scale = if feedforward_scale.is_finite() && feedforward_scale > 0.0 {
            feedforward_scale
        } else if target_rms.is_finite() && target_rms > 0.0 {
            target_rms / self.full_rms
        } else {
            return;
        };
        let ratio = (scale / self.level_scale).powi(2);
        for d in &mut self.drive[self.lo..self.hi] {
            *d *= ratio;
        }
        self.level_scale = scale;
    }
}

export_law!(RatioLaw);
