//! Mixed Mode through the protocol (schema/PROTOCOL.md, "Mixed Mode"): the
//! configuration and its checks, the state and live frames encoded from the
//! controller's status and display, the subscriptions, and the laws' facts for
//! the floor's role and the tones'.

pub mod config;
pub mod subscribe;
pub mod view;

use crate::protocol::{ErrorCode, MethodError};
use crate::random::run::LogEntry;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use view::MixedView;

/// A publish, as the service hears it: the run id and the view published.
pub type Notice = (String, Arc<MixedView>);

/// The service's handle on one configured run.
pub struct MixedRun {
    pub id: String,
    /// The configuration as accepted, for `mixed.reset` and `mixed.config`.
    pub config: Value,
    /// Its derived values, as configure returned them.
    pub derived: Value,
    /// The configuration as parsed.
    pub wire: Arc<config::WireMixed>,
    /// The controller's configuration, as the configure checks built it.
    pub core: Arc<hertzforge_mixed::MixedConfig>,
    /// Both roles' laws as the checks saw them.
    pub law: config::LawFacts,
    view: Mutex<Arc<MixedView>>,
    log: Mutex<Vec<LogEntry>>,
    notify: tokio::sync::mpsc::UnboundedSender<Notice>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

impl MixedRun {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        config: Value,
        derived: Value,
        wire: Arc<config::WireMixed>,
        core: Arc<hertzforge_mixed::MixedConfig>,
        law: config::LawFacts,
        initial: MixedView,
        notify: tokio::sync::mpsc::UnboundedSender<Notice>,
    ) -> MixedRun {
        MixedRun {
            id,
            config,
            derived,
            wire,
            core,
            law,
            view: Mutex::new(Arc::new(initial)),
            log: Mutex::new(Vec::new()),
            notify,
        }
    }

    pub fn view(&self) -> Arc<MixedView> {
        lock(&self.view).clone()
    }

    /// Publish a new view and tell the service.
    pub fn publish(&self, v: MixedView) {
        let v = Arc::new(v);
        *lock(&self.view) = v.clone();
        let _ = self.notify.send((self.id.clone(), v));
    }

    pub fn log_since(&self, since: u64) -> (Vec<LogEntry>, u64) {
        let log = lock(&self.log);
        let next = log.len() as u64;
        let from = (since as usize).min(log.len());
        (log[from..].to_vec(), next)
    }

    pub fn add_log(&self, kind: &'static str, text: impl Into<String>) {
        let mut log = lock(&self.log);
        let seq = log.len() as u64;
        log.push(LogEntry {
            seq,
            at_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
            kind,
            text: text.into(),
        });
    }
}

/// A refusal by name.
pub fn refuse(reason: &'static str, message: impl Into<String>) -> MethodError {
    MethodError::named(ErrorCode::Refused, reason, message)
}

/// A verb asked in a state it does not apply in.
pub fn wrong_state(v: &MixedView, verb: &str) -> MethodError {
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
