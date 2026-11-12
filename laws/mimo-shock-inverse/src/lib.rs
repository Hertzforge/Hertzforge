//! The built-in MIMO Shock law `inverse` as a law file. It is the controller's
//! own [`InverseLaw`]: this crate only carries it across the law interface,
//! with the manifest from the same declaration ([`builtin::INVERSE`]) as the
//! controller's catalog entry.

use hertzforge_law_sdk::shock_mimo::math::inverse::InverseLaw;
use hertzforge_law_sdk::shock_mimo::*;
use hertzforge_law_sdk::{export_shock_mimo_law, Params};

/// `inverse` behind the law interface.
pub struct InverseFile(InverseLaw);

impl ShockMimoLaw for InverseFile {
    fn describe() -> ShockMimoManifest {
        builtin::INVERSE.manifest()
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        InverseLaw::new(setup.to_math(), builtin::inverse_params(&params)?).map(InverseFile)
    }

    fn seed(&mut self, level_db: f64) -> Result<Outputs, String> {
        self.0.seed(level_db).map(Outputs::from_math)
    }

    fn iterate(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.iterate(&inputs.to_math()).map(Outputs::from_math)
    }
}

export_shock_mimo_law!(InverseFile);
