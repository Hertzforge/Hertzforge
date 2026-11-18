//! The run engine's public surface. The engine is not built in this version:
//! [`MimoSineEngine::new`] checks the setup and the law's acceptance of it,
//! and every run function answers [`EngineError::NotBuilt`].

use crate::law::{check_law_accepts, MimoSineLawFactory};
use crate::setup::{MimoSineSetup, SetupError};
use std::fmt;
use std::sync::Arc;

/// Why the engine refused.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineError {
    /// The setup was refused, or the law does not accept it.
    Setup(SetupError),
    /// The run engine is not built in this version.
    NotBuilt,
}

impl EngineError {
    /// The stable name: the setup rule's name, or `not_built`.
    pub fn reason(&self) -> &'static str {
        match self {
            EngineError::Setup(e) => e.rule.as_str(),
            EngineError::NotBuilt => "not_built",
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Setup(e) => write!(f, "the setup was refused: {e}"),
            EngineError::NotBuilt => f.write_str("the MIMO Sine run engine is not built yet"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<SetupError> for EngineError {
    fn from(e: SetupError) -> Self {
        EngineError::Setup(e)
    }
}

/// A MIMO Sine run: its setup and the law that runs it.
pub struct MimoSineEngine {
    setup: MimoSineSetup,
    law: Arc<dyn MimoSineLawFactory>,
}

impl MimoSineEngine {
    /// A run of `setup` under the law `law` builds: the setup is checked, and
    /// the law must accept its shape, its test type and its reference.
    pub fn new(
        setup: MimoSineSetup,
        law: Arc<dyn MimoSineLawFactory>,
    ) -> Result<Self, EngineError> {
        setup.validate()?;
        check_law_accepts(
            &law.manifest(),
            setup.shape(),
            setup.test_type(),
            setup.phase_specified,
        )?;
        Ok(MimoSineEngine { setup, law })
    }

    /// The run's setup.
    pub fn setup(&self) -> &MimoSineSetup {
        &self.setup
    }

    /// The law's factory.
    pub fn law(&self) -> &Arc<dyn MimoSineLawFactory> {
        &self.law
    }

    /// Identify the structure. Not built in this version.
    pub fn identify(&mut self) -> Result<(), EngineError> {
        Err(EngineError::NotBuilt)
    }

    /// Run the test. Not built in this version.
    pub fn run(&mut self) -> Result<(), EngineError> {
        Err(EngineError::NotBuilt)
    }
}
