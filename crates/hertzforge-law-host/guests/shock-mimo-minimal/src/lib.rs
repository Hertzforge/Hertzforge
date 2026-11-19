//! The smallest law file of the `shock-mimo` world: it describes itself, accepts
//! both target kinds and returns silence on every drive. The law host's tests use
//! it to show that a MIMO Shock law file is recognized by its world.

wit_bindgen::generate!({
    path: "../../../../wit/control-law",
    world: "shock-mimo",
});

use exports::hertzforge::control_law::shock_mimo_law::{self, GuestLaw};
use hertzforge::control_law::mimo_shock_types::{
    MimoShockInputs, MimoShockOutputs, MimoShockSetup, TargetKind,
};
use hertzforge::control_law::types::{Manifest, Param};

struct Minimal;

struct Silence {
    samples: usize,
}

impl Silence {
    fn outputs(&self) -> MimoShockOutputs {
        MimoShockOutputs {
            drives: vec![0.0; self.samples],
            delays_s: None,
            values: Vec::new(),
        }
    }
}

impl shock_mimo_law::Guest for Minimal {
    type Law = Silence;
}

impl Guest for Minimal {
    fn describe() -> MimoShockManifest {
        MimoShockManifest {
            law: Manifest {
                name: "silence".into(),
                version: "0.1.0".into(),
                author: String::new(),
                summary: "Returns silence on every drive.".into(),
                params: Vec::new(),
                diagnostics: Vec::new(),
            },
            targets: vec![TargetKind::Waveform, TargetKind::Srs],
        }
    }
}

impl GuestLaw for Silence {
    fn new(setup: MimoShockSetup, _params: Vec<Param>) -> Result<Silence, String> {
        let samples = setup.drives as usize * setup.record_len as usize;
        Ok(Silence { samples })
    }

    fn seed(&self, _level_db: f64) -> Result<MimoShockOutputs, String> {
        Ok(self.outputs())
    }

    fn iterate(&self, _inputs: MimoShockInputs) -> Result<MimoShockOutputs, String> {
        Ok(self.outputs())
    }
}

export!(Minimal);
