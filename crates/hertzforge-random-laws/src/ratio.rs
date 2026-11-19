//! The built-in `ratio` law, native: `hertzforge-law-ratio-math` behind the run
//! core's [`ControlLaw`]. The law file `laws/ratio` runs the same math behind
//! the law interface.

use hertzforge_law_ratio_math::{Measurement, RatioLaw, RatioParams, Report};
use hertzforge_random::control::{
    ControlError, ControlLaw, LawFault, LevelCommand, LoopDiagnostics, LoopInputs, LoopOutputs,
};
use hertzforge_random::service::law::{LawFactory, LawSetup};

/// The per-line ratio law as a native control law.
pub struct NativeRatioLaw(RatioLaw);

impl NativeRatioLaw {
    /// The stable name the law gives itself (its catalog id).
    pub const NAME: &'static str = "ratio";

    fn outputs(&self, r: Report) -> LoopOutputs {
        LoopOutputs {
            drive_psd: self.0.drive().to_vec(),
            level_cmd_db: self.0.level_cmd_db(),
            diagnostics: LoopDiagnostics {
                untrusted_bins: r.untrusted,
                clamped_bins: r.clamped,
                divergent_bins: Vec::new(),
                inband_err_db: r.inband_err_db,
                inband_rms_g: r.inband_rms,
                servo_faulted: false,
            },
        }
    }
}

fn measurement<'a>(i: &LoopInputs<'a>) -> Measurement<'a> {
    Measurement {
        measured_psd: i.measured_psd,
        h_mag2: i.h_mag2,
        coherence: i.coherence,
        notched_bins: i.notched_bins,
        df_hz: i.df_hz,
    }
}

impl ControlLaw for NativeRatioLaw {
    fn init(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        let r = self
            .0
            .seed(&measurement(inputs))
            .map_err(LawFault::Failed)?;
        Ok(self.outputs(r))
    }

    fn iterate(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        let r = self
            .0
            .iterate(&measurement(inputs))
            .map_err(LawFault::Failed)?;
        Ok(self.outputs(r))
    }

    fn retarget_level(&mut self, cmd: LevelCommand) -> Result<LoopOutputs, LawFault> {
        let r = self.0.retarget(cmd.level_db).map_err(LawFault::Failed)?;
        Ok(self.outputs(r))
    }

    fn name(&self) -> &str {
        Self::NAME
    }
}

/// Builds the native ratio law with fixed parameter values.
#[derive(Debug, Clone, Copy)]
pub struct NativeRatioFactory {
    params: RatioParams,
}

impl NativeRatioFactory {
    /// A factory for the ratio law with `params`.
    pub fn new(params: RatioParams) -> Self {
        NativeRatioFactory { params }
    }
}

impl LawFactory for NativeRatioFactory {
    fn build(&self, setup: &LawSetup<'_>) -> Result<Box<dyn ControlLaw + Send>, ControlError> {
        let law = RatioLaw::new(
            setup.target_psd.to_vec(),
            setup.band,
            setup.df_hz,
            self.params,
        )
        .map_err(ControlError::BadConfig)?;
        Ok(Box::new(NativeRatioLaw(law)))
    }
}
