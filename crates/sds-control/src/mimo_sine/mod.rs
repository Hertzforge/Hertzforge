//! MIMO Sine through the protocol (schema/PROTOCOL.md, "MIMO Sine"): the
//! configuration and its checks, the run thread that drives the engine's
//! `MimoSineService`, the state, live and sweep frames, the subscriptions, the
//! law choice and the law check.

pub mod config;
pub mod law_check;
pub mod laws;
pub mod record;
pub mod run;
pub mod subscribe;
pub mod time;
pub mod view;

use crate::protocol::{ErrorCode, MethodError};
use run::{Cmd, RunShared};
use serde_json::Value;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use view::MsView;

/// The service's handle on one configured run. Dropping it ends the run
/// thread, which releases anything the run holds; the service drops a run only
/// when it is not live.
pub struct MsineRun {
    pub id: String,
    pub shared: Arc<RunShared>,
    /// The configuration as accepted, for `mimo_sine.reset` and
    /// `mimo_sine.config`.
    pub config: Value,
    /// Its derived values, as configure returned them.
    pub derived: Value,
    pub wire: Arc<config::WireMsine>,
    pub law: laws::LawFacts,
    tx: Mutex<mpsc::Sender<Cmd>>,
}

impl MsineRun {
    pub fn new(
        id: String,
        shared: Arc<RunShared>,
        tx: mpsc::Sender<Cmd>,
        config: Value,
        derived: Value,
        wire: Arc<config::WireMsine>,
        law: laws::LawFacts,
    ) -> MsineRun {
        MsineRun {
            id,
            shared,
            config,
            derived,
            wire,
            law,
            tx: Mutex::new(tx),
        }
    }

    pub fn view(&self) -> Arc<MsView> {
        self.shared.view()
    }

    pub fn log_since(&self, since: u64) -> (Vec<crate::random::run::LogEntry>, u64) {
        self.shared.log_since(since)
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

/// The law's parameter values in force: its declared defaults with the given
/// values set.
pub fn law_params_json(facts: &laws::LawFacts, given: &serde_json::Map<String, Value>) -> Value {
    use hertzforge_law_host::ParamKind;
    let mut m = serde_json::Map::new();
    for p in &facts.params {
        let v = given.get(&p.key).cloned().unwrap_or_else(|| match &p.kind {
            ParamKind::Number(s) | ParamKind::PerChannel(s) => crate::protocol::num(s.default),
            ParamKind::Integer(s) => Value::from(s.default),
            ParamKind::Toggle(d) => Value::from(*d),
            ParamKind::Choice(s) => Value::from(s.default.as_str()),
            ParamKind::Curve(_) => Value::Null,
        });
        m.insert(p.key.clone(), v);
    }
    Value::Object(m)
}

/// The run's law as the state names it: identity and parameters in force.
pub fn law_state(facts: &laws::LawFacts, given: &serde_json::Map<String, Value>) -> Value {
    serde_json::json!({
        "id": facts.id,
        "name": facts.name,
        "version": facts.version,
        "sha256": if facts.sha256.is_empty() { Value::Null } else { Value::from(facts.sha256.as_str()) },
        "params": law_params_json(facts, given),
    })
}
