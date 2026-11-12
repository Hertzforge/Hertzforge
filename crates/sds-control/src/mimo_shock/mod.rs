//! MIMO Shock through the protocol (schema/PROTOCOL.md, "MIMO Shock"): the
//! configuration, the run on the transient-run rails driving the MIMO Shock
//! engine, its views, the display subscriptions, the law choice and the law
//! check.

pub mod config;
pub mod law_check;
pub mod laws;
pub mod run;
pub mod subscribe;
pub mod view;

use crate::pulse::rails::{Jobs, Shared};
use crate::pulse::ring::PulseRing;
use crate::random::run::LogEntry;
use crate::random::view::State;
use run::Cmd;
use serde_json::Value;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use view::{IdentFigures, LawReading, MsView, PulseFigures};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// What the service reads of a run without its thread: every row, the latest
/// pulses' figures, the identification, the law's last values and when the next
/// pulse is due.
#[derive(Default)]
pub struct RunData {
    rows: Mutex<Vec<Value>>,
    figures: Mutex<PulseRing<Arc<PulseFigures>>>,
    identification: Mutex<Option<Arc<IdentFigures>>>,
    law: Mutex<Vec<LawReading>>,
    next_due: Mutex<Option<Instant>>,
}

impl RunData {
    pub fn add_pulse(&self, row: Value, figures: PulseFigures) {
        let index = row["index"].as_u64().unwrap_or(0);
        lock(&self.figures).push(index, Arc::new(figures));
        lock(&self.rows).push(row);
    }

    /// The rows from index `since`, and the index after the last.
    pub fn rows_since(&self, since: u64) -> (Vec<Value>, u64) {
        let rows = lock(&self.rows);
        let next = rows.len() as u64;
        let from = (since as usize).min(rows.len());
        (rows[from..].to_vec(), next)
    }

    pub fn row(&self, index: u64) -> Option<Value> {
        lock(&self.rows).get(index as usize).cloned()
    }

    pub fn row_count(&self) -> u64 {
        lock(&self.rows).len() as u64
    }

    pub fn figures(&self, index: u64) -> Option<Arc<PulseFigures>> {
        lock(&self.figures).get(index).cloned()
    }

    pub fn last_figures(&self) -> Option<Arc<PulseFigures>> {
        lock(&self.figures).last().cloned()
    }

    pub fn set_identification(&self, f: Option<IdentFigures>) {
        *lock(&self.identification) = f.map(Arc::new);
    }

    pub fn identification(&self) -> Option<Arc<IdentFigures>> {
        lock(&self.identification).clone()
    }

    pub fn set_law(&self, values: Vec<LawReading>) {
        *lock(&self.law) = values;
    }

    pub fn law(&self) -> Vec<LawReading> {
        lock(&self.law).clone()
    }

    pub fn set_next_due(&self, at: Option<Instant>) {
        *lock(&self.next_due) = at;
    }

    /// Seconds until the next pulse is due, 0 once it is; `None` when none is.
    pub fn next_pulse_s(&self) -> Option<f64> {
        lock(&self.next_due).map(|t| t.saturating_duration_since(Instant::now()).as_secs_f64())
    }
}

/// The service's handle on one configured run. Dropping it ends the run thread,
/// which releases anything the run holds; the service drops a run only when it is
/// not live.
pub struct MsRun {
    pub id: String,
    pub shared: Arc<Shared<MsView>>,
    pub data: Arc<RunData>,
    /// The configuration as accepted, for `reset` and `config`.
    pub config: Value,
    /// Its derived values, as configure returned them.
    pub derived: Value,
    /// The targets at the asked rate, for the configuration's views.
    pub derived_full: Arc<config::Derived>,
    pub wire: Arc<config::WireMs>,
    pub jobs: Jobs<Cmd>,
}

impl MsRun {
    /// The view as published, with the clock as of now: a stop asked for shows
    /// as `stopping` until the run has ended.
    pub fn view(&self) -> Arc<MsView> {
        let v = self.shared.view();
        let next = self.data.next_pulse_s();
        let stopping =
            self.shared.signals.cancelled() && matches!(v.state, State::Running | State::Review);
        let running = matches!(v.state, State::Running | State::Stopping);
        if !stopping && !running {
            return v;
        }
        let mut v = (*v).clone();
        if running {
            v.next_pulse_s = next.filter(|_| v.phase == "waiting");
        }
        if stopping {
            v.state = State::Stopping;
            v.phase = "stopping";
            v.next_pulse_s = None;
        }
        Arc::new(v)
    }

    pub fn log_since(&self, since: u64) -> (Vec<LogEntry>, u64) {
        self.shared.log_since(since)
    }

    /// Raise the run's stop (or abort) before the request is queued, so a fire in
    /// flight ends at its next block and nothing more fires.
    pub fn signal(&self, stop_outcome: Option<&'static str>, by: &str) {
        self.shared.set_ended_by(by);
        if let Some(o) = stop_outcome {
            self.shared.set_stop_outcome(o);
        }
        self.shared.signals.raise(stop_outcome.is_none());
    }

    /// Hold firing while nobody holds control, or release it.
    pub fn hold(&self, hold: bool) {
        self.shared.signals.set_hold(hold);
    }
}

/// A run's view as configured: nothing open, the law named.
pub fn configured_view(
    run_id: &str,
    w: &config::WireMs,
    derived: &config::Derived,
    station: crate::shock::view::StationRef,
    facts: &config::LawFacts,
) -> MsView {
    let sim = station.kind == "sim";
    MsView {
        run_id: run_id.to_owned(),
        title: w.title.clone(),
        state: State::Configured,
        phase: "idle",
        plant: if sim {
            w.sim_plant().map(str::to_owned)
        } else {
            None
        },
        station,
        fs_hz: derived.fs_hz,
        record_len: derived.record_len,
        actual: false,
        clock: None,
        target: w.target.kind().as_str(),
        timing: match w.timing {
            hertzforge_mimo_shock::Timing::Free { .. } => "free",
            hertzforge_mimo_shock::Timing::Hold(_) => "hold",
        },
        verdict: w.verdict.as_str(),
        scaled_aborts: w.scaled_aborts,
        mode: w.mode,
        levels_db: w.levels_db.clone(),
        level_db: None,
        rung: None,
        pulses: Default::default(),
        next_pulse_s: None,
        elapsed_s: None,
        identification: None,
        review: None,
        last_pulse: None,
        law: view::LawRef {
            id: facts.id.clone(),
            name: facts.name.clone(),
            version: facts.version.clone(),
            sha256: facts.sha256.clone(),
            params: law_params_json(facts, &w.law_params),
        },
        misses: 0,
        end: None,
        streaming: false,
    }
}

/// The law's parameter values in force: its declared defaults with the given
/// values set.
pub fn law_params_json(facts: &config::LawFacts, given: &serde_json::Map<String, Value>) -> Value {
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
