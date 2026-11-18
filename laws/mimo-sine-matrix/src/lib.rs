//! The built-in MIMO Sine law `matrix` as a law file: the correction through the identification's regularized inverse, carried along a model in a sweep (the default).
//!
//! It is the controller's own law, built by the same constructor
//! ([`builtin::config`]): this crate only carries it across the law interface,
//! with the manifest from the same declaration ([`builtin::MATRIX`]) as the
//! controller's catalog entry.

use hertzforge_law_sdk::sine_mimo::builtin::{self, BuiltinFile};
use hertzforge_law_sdk::sine_mimo::*;
use hertzforge_law_sdk::{export_sine_mimo_law, Params};

/// `matrix` behind the law interface.
pub struct MatrixFile(BuiltinFile);

impl SineMimoLaw for MatrixFile {
    fn describe() -> SineMimoManifest {
        builtin::MATRIX.manifest()
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        BuiltinFile::new(&builtin::MATRIX, &setup, &params).map(MatrixFile)
    }

    fn engage(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.engage(inputs)
    }

    fn tick(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.tick(inputs)
    }
}

export_sine_mimo_law!(MatrixFile);
