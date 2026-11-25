//! The built-in MIMO Shock law `followed` as a law file. It is the controller's
//! own law, built by the same
//! constructor ([`builtin::followed_config`]): this crate only carries it across the law interface,
//! with the manifest from the same declaration ([`builtin::FOLLOWED`]) as the
//! controller's catalog entry.

use hertzforge_law_sdk::shock_mimo::builtin::BuiltLaw;
use hertzforge_law_sdk::shock_mimo::math::LawSteps;
use hertzforge_law_sdk::shock_mimo::*;
use hertzforge_law_sdk::{export_shock_mimo_law, Params};

/// `followed` behind the law interface.
pub struct FollowedFile(BuiltLaw);

impl ShockMimoLaw for FollowedFile {
    fn describe() -> ShockMimoManifest {
        builtin::FOLLOWED.manifest()
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        let ladder = setup.ladder_db.clone();
        builtin::followed_config(&params)?
            .build(setup.to_math(), &ladder)
            .map(FollowedFile)
    }

    fn seed(&mut self, level_db: f64) -> Result<Outputs, String> {
        self.0.seed(level_db).map(Outputs::from_math)
    }

    fn iterate(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.iterate(&inputs.to_math()).map(Outputs::from_math)
    }
}

export_shock_mimo_law!(FollowedFile);

#[cfg(test)]
mod tests;
