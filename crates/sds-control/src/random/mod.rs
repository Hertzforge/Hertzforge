//! Closed-loop Random through the protocol (schema/PROTOCOL.md, "Hardware and Random
//! Control"): the configuration, the run thread that drives the controller's
//! `RandomService`, the station signal for stop and abort, and the views.

pub mod backend;
pub mod config;
pub mod run;
pub mod view;

use crate::protocol::{ErrorCode, MethodError};
use run::{Cmd, LogEntry, RunShared};
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use view::RunView;

/// The service's handle on one configured run. Dropping it ends the run thread,
/// which releases anything the run holds; the service drops a run only when it is
/// not live.
pub struct RandomRun {
    pub id: String,
    pub shared: Arc<RunShared>,
    /// The configuration as accepted, for `random.reset` and `random.config`.
    pub config: serde_json::Value,
    /// Its derived values, as configure returned them.
    pub derived: serde_json::Value,
    /// Its reference and bands on the requested grid, for the reference view.
    pub reference: Arc<view::Lines>,
    tx: Mutex<mpsc::Sender<Cmd>>,
}

impl RandomRun {
    pub fn new(
        id: String,
        shared: Arc<RunShared>,
        tx: mpsc::Sender<Cmd>,
        config: serde_json::Value,
        derived: serde_json::Value,
        reference: Arc<view::Lines>,
    ) -> RandomRun {
        RandomRun {
            id,
            shared,
            config,
            derived,
            reference,
            tx: Mutex::new(tx),
        }
    }

    pub fn view(&self) -> Arc<RunView> {
        self.shared.view()
    }

    pub fn log_since(&self, since: u64) -> (Vec<LogEntry>, u64) {
        self.shared.log_since(since)
    }

    /// Name who is stopping or aborting the run, before the signal is set.
    pub fn ended_by(&self, by: &str) {
        if let Ok(mut b) = self.shared.ended_by.lock() {
            *b = Some(by.to_owned());
        }
    }

    /// Set the station signal (stop or abort) before a request is queued, so a long
    /// controller call ends at the next engine block.
    pub fn signal(&self, s: u8) {
        // An abort always wins over a stop.
        let _ = self.shared.signal.fetch_max(s, Ordering::SeqCst);
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
