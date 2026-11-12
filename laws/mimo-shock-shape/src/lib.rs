//! The built-in MIMO Shock law `shape` as a law file. It is the controller's
//! own [`ShapeLaw`]: this crate only carries it across the law interface,
//! with the manifest from the same declaration ([`builtin::SHAPE`]) as the
//! controller's catalog entry.

use hertzforge_law_sdk::shock_mimo::math::shape::ShapeLaw;
use hertzforge_law_sdk::shock_mimo::*;
use hertzforge_law_sdk::{export_shock_mimo_law, Params};

/// `shape` behind the law interface.
pub struct ShapeFile(ShapeLaw);

impl ShockMimoLaw for ShapeFile {
    fn describe() -> ShockMimoManifest {
        builtin::SHAPE.manifest()
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        ShapeLaw::new(setup.to_math(), builtin::shape_params(&params)?).map(ShapeFile)
    }

    fn seed(&mut self, level_db: f64) -> Result<Outputs, String> {
        self.0.seed(level_db).map(Outputs::from_math)
    }

    fn iterate(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        self.0.iterate(&inputs.to_math()).map(Outputs::from_math)
    }
}

export_shock_mimo_law!(ShapeFile);
