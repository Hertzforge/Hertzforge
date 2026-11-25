//! The built-in MIMO Shock law `followed-hold` as a law file. It is the controller's
//! own law, built by the same
//! constructor ([`builtin::followed_hold_config`]): this crate only carries it across the law interface,
//! with the manifest from the same declaration ([`builtin::FOLLOWED_HOLD`]) as the
//! controller's catalog entry.

use hertzforge_law_sdk::shock_mimo::builtin::BuiltLaw;
use hertzforge_law_sdk::shock_mimo::math::LawSteps;
use hertzforge_law_sdk::shock_mimo::*;
use hertzforge_law_sdk::{export_shock_mimo_law, Params};

/// `followed-hold` behind the law interface.
pub struct FollowedHoldFile(BuiltLaw);

impl ShockMimoLaw for FollowedHoldFile {
    fn describe() -> ShockMimoManifest {
        builtin::FOLLOWED_HOLD.manifest()
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        let ladder = setup.ladder_db.clone();
        builtin::followed_hold_config(&params)?
            .build(setup.to_math(), &ladder)
            .map(FollowedHoldFile)
    }

    fn seed(&mut self, level_db: f64) -> Result<Outputs, String> {
        self.0.seed(level_db).map(Outputs::from_math)
    }

    fn iterate(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.iterate(&inputs.to_math()).map(Outputs::from_math)
    }
}

export_shock_mimo_law!(FollowedHoldFile);

#[cfg(test)]
mod tests;
