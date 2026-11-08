//! Closed-loop Sine through the protocol (schema/PROTOCOL.md, "Sine"): the
//! configuration and its checks, the profile's imports, checks and views, the run
//! thread that drives the controller's `SineService`, the state and live frames,
//! the subscriptions, and the law check.

pub mod config;
pub mod law_check;
pub mod profile;
pub mod run;
pub mod subscribe;
pub mod view;

use crate::protocol::{ErrorCode, MethodError};
use run::{Cmd, RunShared};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use view::SineView;

/// The service's handle on one configured run. Dropping it ends the run thread,
/// which releases anything the run holds; the service drops a run only when it is
/// not live.
pub struct SineRun {
    pub id: String,
    pub shared: Arc<RunShared>,
    /// The configuration as accepted, for `sine.reset` and `sine.config`.
    pub config: serde_json::Value,
    /// Its derived values, as configure returned them.
    pub derived: serde_json::Value,
    /// The configuration as parsed.
    pub wire: Arc<config::WireSine>,
    /// The run's law as its checks saw it.
    pub law: config::LawFacts,
    tx: Mutex<mpsc::Sender<Cmd>>,
}

impl SineRun {
    pub fn new(
        id: String,
        shared: Arc<RunShared>,
        tx: mpsc::Sender<Cmd>,
        config: serde_json::Value,
        derived: serde_json::Value,
        wire: Arc<config::WireSine>,
        law: config::LawFacts,
    ) -> SineRun {
        SineRun {
            id,
            shared,
            config,
            derived,
            wire,
            law,
            tx: Mutex::new(tx),
        }
    }

    pub fn view(&self) -> Arc<SineView> {
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
