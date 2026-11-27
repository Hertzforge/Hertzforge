//! The built-in MIMO Sine law `followed_poles` as a law file: a modal model fitted to the identification with the station's delay taken out, its poles followed through the sweep.
//!
//! It is the controller's own law, built by the same constructor
//! ([`builtin::config`]): this crate only carries it across the law interface,
//! with the manifest from the same declaration ([`builtin::FOLLOWED_POLES`]) as
//! the controller's catalog entry.

use hertzforge_law_sdk::sine_mimo::builtin::{self, BuiltinFile};
use hertzforge_law_sdk::sine_mimo::*;
use hertzforge_law_sdk::{export_sine_mimo_law, Params};

/// `followed_poles` behind the law interface.
pub struct FollowedPolesFile(BuiltinFile);

impl SineMimoLaw for FollowedPolesFile {
    fn describe() -> SineMimoManifest {
        builtin::FOLLOWED_POLES.manifest()
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        BuiltinFile::new(&builtin::FOLLOWED_POLES, &setup, &params).map(FollowedPolesFile)
    }

    fn engage(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.engage(inputs)
    }

    fn tick(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.tick(inputs)
    }
}

export_sine_mimo_law!(FollowedPolesFile);
