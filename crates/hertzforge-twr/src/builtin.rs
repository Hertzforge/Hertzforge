//! The built-in TWR law, `twr_correct`, as a native law: the shared math in
//! `hertzforge_shock_law_math::twr` behind the core's [`TwrLaw`] and
//! [`TwrLawFactory`]. The law file `laws/twr-correct` links the same math, so
//! the two are one implementation.

use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault, LawValue};
use hertzforge_shock_law_math::classical::Step;
use hertzforge_shock_law_math::twr::{self as math, TwrCorrect, TwrCorrectParams};

use crate::control::{PlayInputs, PlayOutputs, TwrLaw, TwrLawFactory, TwrLawSetup};
use crate::law::LawConfig;

/// The factory for the built-in law with its parameter values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TwrCorrectFactory {
    params: TwrCorrectParams,
}

impl TwrCorrectFactory {
    /// The built-in law with `params`.
    pub fn new(params: TwrCorrectParams) -> Self {
        TwrCorrectFactory { params }
    }

    /// The built-in law with the correction settings of a run's configuration
    /// (`beta`, `max_db_per_iter`, `initial_drive_backoff_db`): the law a run
    /// gets when no other factory is installed.
    pub fn from_law_config(law: &LawConfig) -> Self {
        TwrCorrectFactory::new(TwrCorrectParams {
            beta: law.beta,
            max_db_per_iter: law.max_db_per_iter,
            initial_backoff_db: law.initial_drive_backoff_db,
        })
    }

    /// The parameter values it builds the law with.
    pub fn params(&self) -> TwrCorrectParams {
        self.params
    }
}

impl Default for TwrCorrectFactory {
    /// The declared defaults.
    fn default() -> Self {
        TwrCorrectFactory::new(TwrCorrectParams::default())
    }
}

impl TwrLawFactory for TwrCorrectFactory {
    fn build(&self, setup: &TwrLawSetup<'_>) -> Result<Box<dyn TwrLaw + Send>, LawFault> {
        TwrCorrect::new(
            self.params,
            setup.record_len,
            setup.reference,
            setup.plant.inverse,
            setup.plant.trusted,
        )
        .map(|law| Box::new(TwrCorrectLaw { law }) as Box<dyn TwrLaw + Send>)
        .map_err(LawFault::Failed)
    }

    fn describe(&self) -> LawDescriptor {
        LawDescriptor {
            id: math::ID.into(),
            name: math::NAME.into(),
            version: math::VERSION.into(),
            sha256: None,
            params: serde_json::json!({
                "beta": self.params.beta,
                "max_db_per_iter": self.params.max_db_per_iter,
                "initial_backoff_db": self.params.initial_backoff_db,
            }),
        }
    }
}

/// The built-in law for one run.
#[derive(Debug, Clone)]
pub struct TwrCorrectLaw {
    law: TwrCorrect,
}

fn outputs(step: Step) -> PlayOutputs {
    PlayOutputs {
        values: step
            .report
            .values()
            .iter()
            .map(|(k, v)| LawValue::new(*k, *v))
            .collect(),
        drive: step.drive,
    }
}

impl TwrLaw for TwrCorrectLaw {
    fn name(&self) -> &str {
        math::NAME
    }

    fn seed(&mut self, _inputs: &PlayInputs<'_>) -> Result<PlayOutputs, LawFault> {
        self.law.seed().map(outputs).map_err(LawFault::Failed)
    }

    fn iterate(&mut self, inputs: &PlayInputs<'_>) -> Result<PlayOutputs, LawFault> {
        self.law
            .iterate(inputs.response, inputs.limited.then_some(inputs.drive))
            .map(outputs)
            .map_err(LawFault::Failed)
    }
}
