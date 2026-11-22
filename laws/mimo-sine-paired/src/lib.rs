//! The built-in MIMO Sine law `paired` as a law file: each control paired with one drive, following its own ratio of wanted to measured amplitude.
//!
//! It is the controller's own law, built by the same constructor
//! ([`builtin::config`]): this crate only carries it across the law interface,
//! with the manifest from the same declaration ([`builtin::PAIRED`]) as the
//! controller's catalog entry.

use hertzforge_law_sdk::sine_mimo::builtin::{self, BuiltinFile};
use hertzforge_law_sdk::sine_mimo::*;
use hertzforge_law_sdk::{export_sine_mimo_law, Params};

/// `paired` behind the law interface.
pub struct PairedFile(BuiltinFile);

impl SineMimoLaw for PairedFile {
    fn describe() -> SineMimoManifest {
        builtin::PAIRED.manifest()
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        BuiltinFile::new(&builtin::PAIRED, &setup, &params).map(PairedFile)
    }

    fn engage(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.engage(inputs)
    }

    fn tick(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.tick(inputs)
    }
}

export_sine_mimo_law!(PairedFile);
