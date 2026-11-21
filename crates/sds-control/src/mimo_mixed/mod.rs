//! MIMO Mixed Mode through the protocol (schema/PROTOCOL.md, "MIMO Mixed Mode"):
//! the configuration and its checks, the run thread that drives the controller's
//! `MimoMixedService`, the state and frames encoded from its status, the run's
//! record, the subscriptions, and the law check in each role.

pub mod config;
pub mod law_check;
pub mod record;
pub mod run;
pub mod subscribe;
pub mod view;

use crate::protocol::{ErrorCode, MethodError};
use crate::random::run::LogEntry;
use run::{Cmd, RunShared};
use serde_json::{json, Value};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use view::MimoMixedView;

pub use run::Notice;

/// The verbs the controller cannot do, refused by these names (each ends in
/// `_not_built`).
pub const NOT_BUILT: [(&str, &str); 6] = [
    ("mimo_mixed.mode", "mode_not_built"),
    ("mimo_mixed.adjust", "adjust_not_built"),
    ("mimo_mixed.override", "override_not_built"),
    ("mimo_mixed.exclude", "exclude_not_built"),
    (
        "mimo_mixed.identification.save",
        "identification_save_not_built",
    ),
    (
        "mimo_mixed.identification.load",
        "identification_load_not_built",
    ),
];

/// The service's handle on one configured run. Dropping it ends the run thread,
/// which releases anything the run holds; the service drops a run only when it is
/// not live.
pub struct MimoMixedRun {
    pub id: String,
    /// The configuration as accepted, for `mimo_mixed.reset` and `mimo_mixed.config`.
    pub config: Value,
    /// Its derived values, as configure returned them.
    pub derived: Value,
    pub wire: Arc<config::WireMimoMixed>,
    /// The controller's configuration, as the configure checks built it.
    pub core: Arc<hertzforge_mimo_mixed::MimoMixedConfig>,
    pub facts: Arc<config::Derived>,
    pub shared: Arc<RunShared>,
    tx: Mutex<mpsc::Sender<Cmd>>,
}

impl MimoMixedRun {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        config: Value,
        derived: Value,
        wire: Arc<config::WireMimoMixed>,
        core: Arc<hertzforge_mimo_mixed::MimoMixedConfig>,
        facts: Arc<config::Derived>,
        shared: Arc<RunShared>,
        tx: mpsc::Sender<Cmd>,
    ) -> MimoMixedRun {
        MimoMixedRun {
            id,
            config,
            derived,
            wire,
            core,
            facts,
            shared,
            tx: Mutex::new(tx),
        }
    }

    pub fn view(&self) -> Arc<MimoMixedView> {
        self.shared.view()
    }

    pub fn log_since(&self, since: u64) -> (Vec<LogEntry>, u64) {
        self.shared.log_since(since)
    }

    pub fn add_log(&self, kind: &'static str, text: impl Into<String>) {
        self.shared.add_log(kind, text)
    }

    /// Queue a request and wait for its answer, at most `wait`.
    pub async fn ask<T: Send + 'static>(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<T, MethodError>>) -> Cmd,
        wait: Duration,
    ) -> Result<T, MethodError> {
        let (tx, rx) = oneshot::channel();
        self.send(make(tx))?;
        match tokio::time::timeout(wait, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(MethodError::internal(format!(
                "run {} ended before it answered",
                self.id
            ))),
            Err(_) => Err(MethodError::new(
                ErrorCode::Internal,
                format!(
                    "run {} did not answer within {:.0} s; the request stands",
                    self.id,
                    wait.as_secs_f64()
                ),
            )),
        }
    }

    pub fn send(&self, cmd: Cmd) -> Result<(), MethodError> {
        let tx = match self.tx.lock() {
            Ok(t) => t,
            Err(p) => p.into_inner(),
        };
        tx.send(cmd)
            .map_err(|_| MethodError::internal(format!("run {} is no longer running", self.id)))
    }
}

/// A refusal by name.
pub fn refuse(reason: &'static str, message: impl Into<String>) -> MethodError {
    MethodError::named(ErrorCode::Refused, reason, message)
}

/// A verb asked in a state it does not apply in.
pub fn wrong_state(v: &MimoMixedView, verb: &str) -> MethodError {
    refuse(
        "wrong_state",
        format!(
            "{verb} does not apply while the run is {} ({}).",
            v.state.name(),
            v.phase.replace('_', " ")
        ),
    )
    .with_detail(json!({"state": v.state.name(), "phase": v.phase}))
}

/// A verb this module does not build, refused by its name.
pub fn not_built(method: &str) -> Option<MethodError> {
    let (_, reason) = NOT_BUILT.iter().find(|(m, _)| *m == method)?;
    let words = match *reason {
        "mode_not_built" => {
            "The level schedule's mode is fixed by the configuration in MIMO Mixed Mode; \
             configure the schedule's mode instead."
        }
        "adjust_not_built" => {
            "MIMO Mixed Mode's laws take no live parameters; configure the parameters instead."
        }
        "override_not_built" => {
            "MIMO Mixed Mode takes no control override; every control target keeps its \
             reference, and a part is switched with mimo_mixed.part."
        }
        "exclude_not_built" => {
            "MIMO Mixed Mode does not exclude a control mid-run; configure the controls instead."
        }
        "identification_save_not_built" => {
            "MIMO Mixed Mode does not save its identification as an identification set; \
             the run's record keeps it."
        }
        _ => "MIMO Mixed Mode identifies on every run; it cannot load an identification set.",
    };
    Some(refuse(reason, words))
}
