//! The built-in MIMO Sine law `steepest_descent` as a law file: the drive moved down the gradient of the squared control error through the identification.
//!
//! It is the controller's own law, built by the same constructor
//! ([`builtin::config`]): this crate only carries it across the law interface,
//! with the manifest from the same declaration ([`builtin::STEEPEST_DESCENT`]) as the
//! controller's catalog entry.

use hertzforge_law_sdk::sine_mimo::builtin::{self, BuiltinFile};
use hertzforge_law_sdk::sine_mimo::*;
use hertzforge_law_sdk::{export_sine_mimo_law, Params};

/// `steepest_descent` behind the law interface.
pub struct SteepestDescentFile(BuiltinFile);

impl SineMimoLaw for SteepestDescentFile {
    fn describe() -> SineMimoManifest {
        builtin::STEEPEST_DESCENT.manifest()
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        BuiltinFile::new(&builtin::STEEPEST_DESCENT, &setup, &params).map(SteepestDescentFile)
    }

    fn engage(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.engage(inputs)
    }

    fn tick(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.tick(inputs)
    }
}

export_sine_mimo_law!(SteepestDescentFile);
