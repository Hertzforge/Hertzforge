//! Per-line ratio correction: a single-output Random control law.
//!
//! The math is `hertzforge-law-ratio-math`, the same code the controller runs
//! as its built-in native `ratio` law; this crate only wraps it in the law
//! interface. See that crate for what the law does.
//!
//! The controller applies its own protections to the drive this law returns.

use hertzforge_law_ratio_math::{self as ratio, Measurement, RatioParams, Report};
use hertzforge_law_sdk::{
    band_range, export_law, DiagnosticSpec, DiagnosticValue, LoopDiagnostics, LoopInputs,
    LoopOutputs, Manifest, NumberSpec, ParamKind, ParamSpec, Params, RandomLaw, Setup,
};

/// The law's state for one run.
pub struct RatioLaw(ratio::RatioLaw);

fn measurement(i: &LoopInputs) -> Measurement<'_> {
    Measurement {
        measured_psd: &i.measured_psd,
        h_mag2: &i.h_mag2,
        coherence: &i.coherence,
        notched_bins: &i.notched_bins,
        df_hz: i.df_hz,
    }
}

fn lines(v: Vec<usize>) -> Vec<u32> {
    // Every line index is below the grid length, which the interface carries
    // as a u32.
    v.into_iter().map(|k| k as u32).collect()
}

impl RatioLaw {
    fn outputs(&self, r: Report) -> LoopOutputs {
        let values = vec![
            DiagnosticValue {
                key: ratio::DIAGNOSTICS[0].key.into(),
                value: r.untrusted.len() as f64,
            },
            DiagnosticValue {
                key: ratio::DIAGNOSTICS[1].key.into(),
                value: r.clamped.len() as f64,
            },
        ];
        LoopOutputs {
            drive_psd: self.0.drive().to_vec(),
            level_cmd_db: self.0.level_cmd_db(),
            diagnostics: LoopDiagnostics {
                untrusted_bins: lines(r.untrusted),
                clamped_bins: lines(r.clamped),
                divergent_bins: Vec::new(),
                inband_err_db: r.inband_err_db,
                inband_rms_g: r.inband_rms,
                servo_faulted: false,
                values,
            },
        }
    }
}

impl RandomLaw for RatioLaw {
    fn describe() -> Manifest {
        Manifest {
            name: ratio::NAME.into(),
            version: ratio::VERSION.into(),
            author: ratio::AUTHOR.into(),
            summary: ratio::SUMMARY.into(),
            params: ratio::PARAMS
                .iter()
                .map(|p| ParamSpec {
                    key: p.key.into(),
                    label: p.label.into(),
                    help: p.help.into(),
                    kind: ParamKind::Number(NumberSpec {
                        unit: p.unit.into(),
                        min: p.min,
                        max: p.max,
                        step: p.step,
                        default: p.default,
                    }),
                })
                .collect(),
            diagnostics: ratio::DIAGNOSTICS
                .iter()
                .map(|d| DiagnosticSpec {
                    key: d.key.into(),
                    label: d.label.into(),
                    unit: d.unit.into(),
                    help: d.help.into(),
                })
                .collect(),
        }
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        let mut p = RatioParams::default();
        for decl in ratio::PARAMS {
            p.set(decl.key, params.number(decl.key)?)?;
        }
        let r = band_range(setup.band);
        ratio::RatioLaw::new(setup.target_psd, (r.start, r.end), setup.df_hz, p).map(RatioLaw)
    }

    fn seed(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, String> {
        let r = self.0.seed(&measurement(inputs))?;
        Ok(self.outputs(r))
    }

    fn iterate(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, String> {
        let r = self.0.iterate(&measurement(inputs))?;
        Ok(self.outputs(r))
    }

    fn retarget_level(&mut self, level_db: f64, _target_rms: f64) -> Result<LoopOutputs, String> {
        let r = self.0.retarget(level_db)?;
        Ok(self.outputs(r))
    }
}

export_law!(RatioLaw);
