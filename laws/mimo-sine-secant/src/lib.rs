//! The built-in MIMO Sine law `secant` as a law file: the secant law: a complex gain found from a probe and its measured response.
//!
//! It is the controller's own law, built by the same constructor
//! ([`builtin::config`]): this crate only carries it across the law interface,
//! with the manifest from the same declaration ([`builtin::SECANT`]) as the
//! controller's catalog entry.

use hertzforge_law_sdk::sine_mimo::builtin::{self, BuiltinFile};
use hertzforge_law_sdk::sine_mimo::*;
use hertzforge_law_sdk::{export_sine_mimo_law, Params};

/// `secant` behind the law interface.
pub struct SecantFile(BuiltinFile);

impl SineMimoLaw for SecantFile {
    fn describe() -> SineMimoManifest {
        builtin::SECANT.manifest()
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        BuiltinFile::new(&builtin::SECANT, &setup, &params).map(SecantFile)
    }

    fn engage(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.engage(inputs)
    }

    fn tick(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.tick(inputs)
    }
}

export_sine_mimo_law!(SecantFile);
