//! The native MIMO Shock law traits.
//!
//! A law is built once per run, after identification, by a
//! [`MimoShockLawFactory`] (which holds the law's parameter values), seeded for
//! the first level, then called once after each fired, measured and judged pulse
//! with the next level already scheduled. A law file of the `shock-mimo` world
//! is adapted to the same traits. The guards (budget, misses, output checks,
//! drive ceilings, the growth cap, the predicted response) are the core's.

use crate::pulse::{PulseInputs, PulseOutputs};
use crate::setup::{Setup, SetupError, SetupRule, TargetKind};
use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault};
use serde::{Deserialize, Serialize};

/// What a law says about itself besides its descriptor: the target kinds it
/// accepts and the value keys it may report. Mirrors `mimo-shock-manifest`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawManifest {
    /// The target kinds the law accepts: at least one, each at most once.
    pub targets: Vec<TargetKind>,
    /// The keys of the values the law may report.
    pub values: Vec<String>,
}

impl LawManifest {
    /// Whether the law accepts a target of `kind`.
    pub fn accepts(&self, kind: TargetKind) -> bool {
        self.targets.contains(&kind)
    }
}

/// A MIMO Shock law. Its state lives in the value.
pub trait MimoShockLaw {
    /// The law's name, for the log and the report.
    fn name(&self) -> &str;

    /// The first drives, before any pulse has fired, for the ladder's first
    /// level `level_db`. A miss here fires nothing.
    fn seed(&mut self, level_db: f64) -> Result<PulseOutputs, LawFault>;

    /// The next drives, after a pulse was fired, measured and judged and the
    /// next level (`inputs.next_level_db`) scheduled.
    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, LawFault>;
}

/// Builds the law for a run. Called once per run, after identification.
pub trait MimoShockLawFactory: Send + Sync {
    /// Build a law for `setup`, or say why it cannot be built (the run then
    /// ends `law_fault` before any pulse).
    fn build(&self, setup: &Setup) -> Result<Box<dyn MimoShockLaw + Send>, LawFault>;

    /// The law as the run's records name it.
    fn describe(&self) -> LawDescriptor;

    /// The target kinds it accepts and the values it may report.
    fn manifest(&self) -> LawManifest;
}

/// Refuse a run whose target kind the law does not accept, before anything is
/// built: [`SetupRule::TargetNotAccepted`].
pub fn check_law_accepts(manifest: &LawManifest, setup: &Setup) -> Result<(), SetupError> {
    let kind = setup.target.kind();
    if manifest.accepts(kind) {
        Ok(())
    } else {
        Err(SetupError::new(
            SetupRule::TargetNotAccepted,
            format!(
                "the law accepts {} targets, not {}",
                manifest
                    .targets
                    .iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(" and "),
                kind.as_str()
            ),
        ))
    }
}
