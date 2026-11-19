//! The default single-output Random law, H-inverse with a level servo and a
//! per-line shape trim, as a law file.
//!
//! This is the controller's own [`HinvLaw`], linked without the run service:
//! the law the controller runs natively and this law file are one
//! implementation. This crate only carries values across the law interface.
//! The parameters are the law's operator knobs ([`HinvConfig::KNOBS`]), the
//! same declaration the controller's built-in entry is described by.

use hertzforge_law_sdk::{
    band_range, export_law, IntegerSpec, LoopDiagnostics, LoopInputs, LoopOutputs, Manifest,
    NumberSpec, ParamKind, ParamSpec, ParamValue, Params, RandomLaw, Setup,
};
use hertzforge_random::control::{
    ControlLaw, HinvConfig, HinvLaw, KnobKind, LevelCommand, LoopInputs as CoreInputs,
    LoopOutputs as CoreOutputs,
};

/// The law's state for one run.
pub struct Hinv(HinvLaw);

fn core_inputs(i: &LoopInputs) -> CoreInputs<'_> {
    let r = band_range(i.band);
    CoreInputs {
        target_psd: &i.target_psd,
        measured_psd: &i.measured_psd,
        h_mag2: &i.h_mag2,
        coherence: &i.coherence,
        current_level_db: i.current_level_db,
        df_hz: i.df_hz,
        notched_bins: &i.notched_bins,
        band: (r.start, r.end),
        dof: i.dof,
    }
}

fn lines(v: Vec<usize>) -> Vec<u32> {
    // Every line index is below the grid length, which the interface carries
    // as a u32.
    v.into_iter().map(|k| k as u32).collect()
}

fn outputs(o: CoreOutputs) -> LoopOutputs {
    let d = o.diagnostics;
    LoopOutputs {
        drive_psd: o.drive_psd,
        level_cmd_db: o.level_cmd_db,
        diagnostics: LoopDiagnostics {
            untrusted_bins: lines(d.untrusted_bins),
            clamped_bins: lines(d.clamped_bins),
            divergent_bins: lines(d.divergent_bins),
            inband_err_db: d.inband_err_db,
            inband_rms_g: d.inband_rms_g,
            servo_faulted: d.servo_faulted,
            values: Vec::new(),
        },
    }
}

impl RandomLaw for Hinv {
    fn describe() -> Manifest {
        let defaults = HinvConfig::default();
        Manifest {
            name: HinvLaw::TITLE.into(),
            version: HinvLaw::VERSION.into(),
            author: HinvLaw::AUTHOR.into(),
            summary: HinvLaw::SUMMARY.into(),
            params: HinvConfig::KNOBS
                .iter()
                .map(|k| {
                    let default = defaults.knob(k.key).unwrap_or(k.min);
                    ParamSpec {
                        key: k.key.into(),
                        label: k.label.into(),
                        help: k.help.into(),
                        kind: match k.kind {
                            KnobKind::Number => ParamKind::Number(NumberSpec {
                                unit: k.unit.into(),
                                min: k.min,
                                max: k.max,
                                step: k.step,
                                default,
                            }),
                            // Whole-number knobs are declared with whole bounds.
                            KnobKind::Integer => ParamKind::Integer(IntegerSpec {
                                unit: k.unit.into(),
                                min: k.min as i64,
                                max: k.max as i64,
                                step: k.step as i64,
                                default: default as i64,
                            }),
                        },
                    }
                })
                .collect(),
            diagnostics: Vec::new(),
        }
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        let mut cfg = HinvConfig::default();
        for p in &params.0 {
            let value = match p.value {
                ParamValue::Number(v) => v,
                ParamValue::Integer(v) => v as f64,
                _ => return Err(format!("parameter `{}` is not a number", p.key)),
            };
            cfg.set_knob(&p.key, value).map_err(|e| e.to_string())?;
        }
        let r = band_range(setup.band);
        HinvLaw::new(cfg, &setup.target_psd, (r.start, r.end), setup.df_hz)
            .map(Hinv)
            .map_err(|e| e.to_string())
    }

    fn seed(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, String> {
        self.0
            .init(&core_inputs(inputs))
            .map(outputs)
            .map_err(|e| e.to_string())
    }

    fn iterate(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, String> {
        self.0
            .iterate(&core_inputs(inputs))
            .map(outputs)
            .map_err(|e| e.to_string())
    }

    fn retarget_level(&mut self, level_db: f64, target_rms: f64) -> Result<LoopOutputs, String> {
        self.0
            .retarget_level(LevelCommand {
                level_db,
                target_rms,
            })
            .map(outputs)
            .map_err(|e| e.to_string())
    }
}

export_law!(Hinv);
