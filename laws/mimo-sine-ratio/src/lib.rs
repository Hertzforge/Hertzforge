//! The built-in MIMO Sine law `ratio` as a law file: each control's amplitude ratio applied to its reference through the identification's inverse.
//!
//! It is the controller's own law, built by the same constructor
//! ([`builtin::config`]): this crate only carries it across the law interface,
//! with the manifest from the same declaration ([`builtin::RATIO`]) as the
//! controller's catalog entry.

use hertzforge_law_sdk::sine_mimo::builtin::{self, BuiltinFile};
use hertzforge_law_sdk::sine_mimo::*;
use hertzforge_law_sdk::{export_sine_mimo_law, Params};

/// `ratio` behind the law interface.
pub struct RatioFile(BuiltinFile);

impl SineMimoLaw for RatioFile {
    fn describe() -> SineMimoManifest {
        builtin::RATIO.manifest()
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        BuiltinFile::new(&builtin::RATIO, &setup, &params).map(RatioFile)
    }

    fn engage(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.engage(inputs)
    }

    fn tick(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.tick(inputs)
    }
}

export_sine_mimo_law!(RatioFile);
