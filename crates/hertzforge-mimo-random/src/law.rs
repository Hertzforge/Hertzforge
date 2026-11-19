//! Building the run's law and calling it under the core's guards.
//!
//! A law comes from a [`MimoLawFactory`] (the catalog's, for a built-in law or a
//! law file). Every call, the seed included, is guarded the way the single-output
//! core guards its law: the call runs under `catch_unwind`, it is timed on an
//! injectable [`LawClock`] against its budget, and an error, a panic or a late
//! answer is a [`Miss`]. The output checks of [`crate::guards`] run after.

use crate::report::MimoFaultReason;
use hertzforge_law_host::validate::resolve_params;
use hertzforge_random::service::law::LawClock;
use hertzforge_random_laws::{
    builtin_mimo_manifest, BuiltinMimo, BuiltinMimoFactory, ChoiceError, LawChoice, MimoLawFactory,
};
use hertzforge_sdm::law::{LawError, MimoControlLaw, MimoLoopOutputs};
use std::sync::Arc;

/// One law call whose output is not used.
#[derive(Debug, Clone, PartialEq)]
pub struct Miss {
    pub reason: MimoFaultReason,
    pub detail: String,
}

impl Miss {
    /// The miss in a sentence.
    pub fn words(&self) -> String {
        if self.detail.is_empty() {
            self.reason.words().to_string()
        } else {
            format!("{} ({})", self.reason.words(), self.detail)
        }
    }
}

/// The factory for a built-in law chosen by id, with its parameter values
/// checked against its manifest. An imported law file is resolved through the
/// law catalog (`LawCatalog::resolve_mimo`) and handed to the service directly.
pub fn builtin_factory(
    choice: &LawChoice,
    controls: usize,
) -> Result<Arc<dyn MimoLawFactory>, ChoiceError> {
    let law = BuiltinMimo::from_id(&choice.id).ok_or_else(|| {
        ChoiceError::UnknownLaw(format!(
            "{} (not a built-in law; import the law file and choose it from the law catalog)",
            choice.id
        ))
    })?;
    let manifest = builtin_mimo_manifest(law);
    let params = resolve_params(&manifest.law, &choice.params, controls).map_err(|e| match e {
        hertzforge_law_host::LawError::ParamInvalid { key, reason } => {
            ChoiceError::BadParam { key, reason }
        }
        other => ChoiceError::BadConfig(other.to_string()),
    })?;
    Ok(Arc::new(BuiltinMimoFactory::new(law, params)))
}

/// Call the law, guarded: contained, timed against `budget_s` on `clock`.
pub fn guarded_call<F>(
    law: &mut (dyn MimoControlLaw + Send),
    clock: &dyn LawClock,
    budget_s: f64,
    call: F,
) -> Result<MimoLoopOutputs, Miss>
where
    F: FnOnce(&mut (dyn MimoControlLaw + Send)) -> Result<MimoLoopOutputs, LawError>,
{
    let started = clock.now_s();
    // A panic inside the law is the law's fault, never the run's. The workspace
    // keeps `panic = "unwind"` for exactly this containment.
    let out = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call(law))) {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            return Err(Miss {
                reason: MimoFaultReason::Error,
                detail: e.0,
            })
        }
        Err(_) => {
            return Err(Miss {
                reason: MimoFaultReason::Panicked,
                detail: String::new(),
            })
        }
    };
    let took = clock.now_s() - started;
    if took.is_nan() || took > budget_s {
        return Err(Miss {
            reason: MimoFaultReason::OverBudget,
            detail: format!("{:.0} ms against {:.0} ms", took * 1e3, budget_s * 1e3),
        });
    }
    Ok(out)
}
