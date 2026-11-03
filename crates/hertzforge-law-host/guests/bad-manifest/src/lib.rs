//! A law whose manifest has an empty name. Everything else is well formed,
//! so the only reason to refuse it is the manifest rule.

use hertzforge_law_sdk::{
    export_law, no_diagnostics, LoopInputs, LoopOutputs, Manifest, Params, RandomLaw, Setup,
};

/// Returns a zero drive.
pub struct Nameless {
    n: usize,
}

impl RandomLaw for Nameless {
    fn describe() -> Manifest {
        Manifest {
            name: String::new(),
            version: "0.0.0".into(),
            author: String::new(),
            summary: "A fixture whose manifest the host must refuse.".into(),
            params: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn new(setup: Setup, _params: Params) -> Result<Self, String> {
        Ok(Nameless {
            n: setup.target_psd.len(),
        })
    }

    fn seed(&mut self, _inputs: &LoopInputs) -> Result<LoopOutputs, String> {
        Ok(self.zero())
    }

    fn iterate(&mut self, _inputs: &LoopInputs) -> Result<LoopOutputs, String> {
        Ok(self.zero())
    }

    fn retarget_level(&mut self, _level_db: f64, _target_rms: f64) -> Result<LoopOutputs, String> {
        Ok(self.zero())
    }
}

impl Nameless {
    fn zero(&self) -> LoopOutputs {
        LoopOutputs {
            drive_psd: vec![0.0; self.n],
            level_cmd_db: 0.0,
            diagnostics: no_diagnostics(),
        }
    }
}

export_law!(Nameless);
