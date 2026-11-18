//! The run engine's public surface: [`MimoSineEngine::new`] checks the setup
//! and the law's acceptance of it; with the `service` feature,
//! [`MimoSineEngine::service`] hands the checked run to a station as a
//! [`crate::service::MimoSineService`], which identifies, engages and runs it.

use crate::law::{check_law_accepts, MimoSineLawFactory};
use crate::setup::{MimoSineSetup, SetupError};
use std::fmt;
use std::sync::Arc;

/// Why the engine refused.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineError {
    /// The setup was refused, or the law does not accept it.
    Setup(SetupError),
}

impl EngineError {
    /// The stable name: the setup rule's name.
    pub fn reason(&self) -> &'static str {
        match self {
            EngineError::Setup(e) => e.rule.as_str(),
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Setup(e) => write!(f, "the setup was refused: {e}"),
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

    /// The run on a station: its backend and channel table, with `opts`.
    #[cfg(feature = "service")]
    pub fn service(
        self,
        table: hertzforge_channel_table::ChannelTable,
        backend: Box<dyn hertzforge_hal::Backend>,
        opts: crate::service::EngineOptions,
    ) -> Result<crate::service::MimoSineService, crate::service::ServiceError> {
        crate::service::MimoSineService::new(self.setup, table, backend, self.law, opts)
    }
}
