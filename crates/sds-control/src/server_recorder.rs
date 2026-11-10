//! The Recorder methods of the [`Service`] (schema/PROTOCOL.md, "Recorder"): a
//! configuration and its run beside the other modules' slots, under the same run
//! exclusion and controller rules, around the core's service.

use super::{Conn, Feed, Out, Service, DATA_SLACK, LIVE_TICK};
use crate::codec::{self, Arrays};
use crate::explorer::view::StationRef;
use crate::protocol::{self, ErrorCode, MethodError};
use crate::pulse::rails::Shared;
use crate::recorder::config::{self, CheckContext, Derived, RecorderStation, WireRecorder};
use crate::recorder::run::{wrong_state, RecorderRun, StartContext};
use crate::recorder::sim_signal;
use crate::recorder::subscribe::{self, DataContext, Subscription};
use crate::recorder::view::{self, RecorderView, State};
use crate::station::{self, Selection, StationPlan};
use crate::table;
use hertzforge_channel_table::ChannelTable;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a verb waits for the core: the pump holds its lock for one batch and one
/// analysis pass, so this bounds a stuck station, not a busy one.
const VERB_WAIT: Duration = Duration::from_secs(5);
/// How often `recorder.live` goes to the connections without a subscription.
const LIVE_PERIOD: Duration = Duration::from_millis(100);
/// How long the shutdown waits for the controlled stop before it aborts.
pub(super) const SHUTDOWN_PATIENCE: Duration = Duration::from_secs(10);

/// One connection's Recorder subscription and when it was last sent a frame.
pub struct RecorderDisplay {
    sub: Arc<Subscription>,
    last: Option<Instant>,
}

fn not_configured() -> MethodError {
    MethodError::named(
        ErrorCode::Refused,
        "not_configured",
        "The Recorder is not configured; configure a test first.",
    )
}

/// Run `f` on the blocking pool, waiting at most `wait`.
async fn blocking<T: Send + 'static>(
    wait: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, MethodError> {
    match tokio::time::timeout(wait, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(t)) => Ok(t),
        Ok(Err(e)) => Err(MethodError::internal(format!("the run's verb failed: {e}"))),
        Err(_) => Err(MethodError::internal(
            "the run did not answer in time; the station may be stuck in a read. Abort stays \
             open.",
        )),
    }
}

fn since_param(params: &Map<String, Value>) -> Result<u64, MethodError> {
    match params.get("since") {
        None | Some(Value::Null) => Ok(0),
        Some(v) => v
            .as_u64()
            .ok_or_else(|| MethodError::bad_params("since", "\"since\" is a whole number from 0")),
    }
}

/// Refuse any parameter of a verb that takes none.
fn no_params(params: &Map<String, Value>, verb: &str) -> Result<(), MethodError> {
    match params.keys().next() {
        None => Ok(()),
        Some(k) => Err(MethodError::bad_params(
            k,
            format!("{verb} takes no parameters, not \"{k}\""),
        )),
    }
}

/// The signal the simulator's inputs carry for a Recorder run. A test may install
/// another through [`Service::set_recorder_signal`]; the wire offers none.
pub(super) type SignalSlot = std::sync::Mutex<Option<sim_signal::Signal>>;

impl Service {
    pub(super) fn recorder_displays(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<u64, RecorderDisplay>> {
        match self.recorder_displays.lock() {
            Ok(d) => d,
            Err(p) => p.into_inner(),
        }
    }

    /// Install the signal the simulator's inputs carry for the next Recorder start
    /// (the stated source when none is installed). For tests: the wire offers none.
    pub fn set_recorder_signal(&self, signal: Option<sim_signal::Signal>) {
        *match self.recorder_signal.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        } = signal;
    }

    fn recorder_station(inner: &super::Inner) -> (StationRef, RecorderStation) {
        let selected = inner.stations.selected().clone();
        let kind = if selected == Selection::Sim {
            RecorderStation::Sim
        } else {
            RecorderStation::Ni
        };
        let r = StationRef {
            kind: if selected == Selection::Sim {
                "sim"
            } else {
                "ni"
            },
            class: station::class_name(inner.stations.class()),
            devices: match &selected {
                Selection::Sim => Vec::new(),
                Selection::Ni { devices } => devices
                    .iter()
                    .map(|id| {
                        let sn = inner
                            .stations
                            .devices()
                            .iter()
                            .find(|d| &d.id == id)
                            .map(|d| d.serial.clone())
                            .unwrap_or_default();
                        (id.clone(), sn)
                    })
                    .collect(),
            },
        };
        (r, kind)
    }

    /// The local data root the run folders go under.
    fn recorder_root(&self) -> std::path::PathBuf {
        self.opts
            .capture_root
            .clone()
            .unwrap_or_else(|| self.opts.data_root.clone())
    }

    /// Parse and check `config` against the present table and station.
    async fn recorder_check(
        &self,
        config: &Value,
    ) -> Result<
        (
            WireRecorder,
            table::Report,
            Option<Derived>,
            ChannelTable,
            StationRef,
            RecorderStation,
        ),
        MethodError,
    > {
        let w = config::parse(config)?;
        let free = hertzforge_capture::free_space_bytes_for(&self.recorder_root()).ok();
        let inner = self.inner.lock().await;
        let tbl = inner.table.table.clone();
        let mut report = table::validate(&tbl, &inner.ctx());
        // A drive row is not an error for a Recorder run: it is not opened.
        report
            .warnings
            .retain(|i| i.reason != "drive_channel_present");
        let (station, kind) = Self::recorder_station(&inner);
        let (r, derived) = w.check(&CheckContext {
            table: &tbl,
            station: kind,
            free_bytes: free,
        });
        report.errors.extend(r.errors);
        report.warnings.extend(r.warnings);
        let derived = if report.errors.is_empty() {
            derived
        } else {
            None
        };
        Ok((w, report, derived, tbl, station, kind))
    }

    pub(super) async fn recorder_configure(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let config = params
            .get("config")
            .filter(|c| c.is_object())
            .ok_or_else(|| {
                MethodError::bad_params("config", "Configuring needs a test configuration.")
            })?;
        let dry_run = protocol::param_bool(params, "dry_run")?.unwrap_or(false);
        let (w, report, derived, tbl, station, kind) = self.recorder_check(config).await?;
        if dry_run {
            return Ok(json!({
                "errors": report.errors.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
                "warnings": report.warnings.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
                "derived": derived.as_ref().map_or(Value::Null, Derived::to_json),
            }));
        }
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.run_active()?;
        if inner.sim_running() {
            return Err(MethodError::named(
                ErrorCode::Refused,
                "run_active",
                "A simulation is running; stop it first.",
            ));
        }
        let Some(derived) = derived else {
            return Err(config::config_invalid(&report));
        };
        let run = self.new_recorder_run(&mut inner, w, derived, &tbl, station, kind);
        Ok(json!({
            "state": view::state_json(Some(&run.view())),
            "derived": run.derived,
            "warnings": report.warnings.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
        }))
    }

    /// Create a run (replacing the one before).
    fn new_recorder_run(
        &self,
        inner: &mut super::Inner,
        w: WireRecorder,
        derived: Derived,
        tbl: &ChannelTable,
        station: StationRef,
        kind: RecorderStation,
    ) -> Arc<RecorderRun> {
        inner.recorder_runs += 1;
        let run_id = format!("recorder-{:04}", inner.recorder_runs);
        let initial = view::configured_view(&run_id, &w, &derived, station, tbl);
        let shared = Arc::new(Shared::new(initial.clone(), self.recorder_notify.clone()));
        let title = w.title.clone();
        let run = Arc::new(RecorderRun::new(
            run_id.clone(),
            shared.clone(),
            Arc::new(w),
            derived,
            kind,
        ));
        shared.add_log(
            "state",
            format!("configured \"{title}\" on {}", initial.station.class),
        );
        inner.recorder = Some(run.clone());
        let _ = self.recorder_notify.send((run_id, Arc::new(initial)));
        run
    }

    pub(super) async fn recorder_config(&self) -> Value {
        let inner = self.inner.lock().await;
        match &inner.recorder {
            None => json!({"run_id": null, "config": null, "derived": null}),
            Some(run) => json!({"run_id": run.id, "config": run.config, "derived": run.derived}),
        }
    }

    async fn controlled_recorder(&self, conn: &Conn) -> Result<Arc<RecorderRun>, MethodError> {
        let inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.recorder.clone().ok_or_else(not_configured)
    }

    pub(super) async fn recorder_start(
        self: &Arc<Self>,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        no_params(params, "recorder.start")?;
        let (run, ctx) = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.recorder.clone().ok_or_else(not_configured)?;
            let v = run.view();
            if v.state == State::Finished {
                return Err(MethodError::named(
                    ErrorCode::Refused,
                    "run_finished",
                    "This run has finished; reset or configure again to run.",
                )
                .with_detail(json!({"outcome": v.end.as_ref().map(|e| e.outcome)})));
            }
            if v.state.is_live() || run.started() {
                return Err(wrong_state(&v, "recorder.start"));
            }
            inner.run_active()?;
            if inner.sim_running() {
                return Err(MethodError::named(
                    ErrorCode::Refused,
                    "run_active",
                    "A simulation is running; stop it first.",
                ));
            }
            let report = table::validate(&inner.table.table, &inner.ctx());
            if !report.errors.is_empty() {
                return Err(super::table_invalid(
                    &report,
                    "on this station, so the run cannot start",
                ));
            }
            // The table as it is now must still carry the run.
            let (_, kind) = Self::recorder_station(&inner);
            let free = hertzforge_capture::free_space_bytes_for(&self.recorder_root()).ok();
            let (r, d) = run.wire.check(&CheckContext {
                table: &inner.table.table,
                station: kind,
                free_bytes: free,
            });
            if !r.errors.is_empty() {
                return Err(config::config_invalid(&r));
            }
            // The free space must be over the floor now; no folder is made otherwise.
            if let (Some(d), Some(free)) = (d, free) {
                if free < d.floor_bytes {
                    return Err(MethodError::named(
                        ErrorCode::Refused,
                        "not_enough_space",
                        format!(
                            "The data volume has {free} bytes free, under the run's floor of {} \
                             bytes; free space or lower capture.keep_free_gb.",
                            d.floor_bytes
                        ),
                    )
                    .with_detail(json!({"free_bytes": free, "floor_bytes": d.floor_bytes})));
                }
            }
            let fs_hz = run.wire.fs_hz;
            let plan = match inner.stations.selected() {
                Selection::Sim => StationPlan::Sim { fs_hz },
                Selection::Ni { devices } => {
                    let (inputs, _) = hertzforge_station::physical_from_table(&inner.table.table);
                    StationPlan::Ni {
                        devices: devices.clone(),
                        inputs,
                        // Capture only: no output is opened.
                        outputs: Vec::new(),
                        fs_hz,
                    }
                }
            };
            run.mark_starting();
            let signal = match self.recorder_signal.lock() {
                Ok(g) => g.clone(),
                Err(p) => p.into_inner().clone(),
            }
            .unwrap_or_else(sim_signal::stated);
            let ctx = StartContext {
                table: inner.table.table.clone(),
                station: plan,
                scanner: self.opts.scanner.clone(),
                data_root: self.opts.data_root.clone(),
                records_root: self.recorder_root(),
                bind: self.opts.bind.clone(),
                signal: Some(signal),
            };
            (run, ctx)
        };
        let r = run.clone();
        let started = tokio::task::spawn_blocking(move || r.start(ctx))
            .await
            .map_err(|e| MethodError::internal(format!("the start failed: {e}")))?;
        self.spawn_recorder_live(run.clone());
        started?;
        Ok(view::state_json(Some(&run.view())))
    }

    /// A verb on the run in progress, on the blocking pool.
    async fn recorder_verb(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
        verb: &'static str,
        f: fn(&RecorderRun) -> Result<Value, MethodError>,
    ) -> Result<Value, MethodError> {
        no_params(params, verb)?;
        let run = self.controlled_recorder(conn).await?;
        let r = run.clone();
        let answer = blocking(VERB_WAIT, move || f(&r)).await??;
        Ok(if answer.is_null() {
            view::state_json(Some(&run.view()))
        } else {
            answer
        })
    }

    pub(super) async fn recorder_arm(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        self.recorder_verb(conn, params, "recorder.arm", RecorderRun::arm)
            .await
    }

    pub(super) async fn recorder_disarm(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        self.recorder_verb(conn, params, "recorder.disarm", RecorderRun::disarm)
            .await
    }

    pub(super) async fn recorder_record(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        self.recorder_verb(conn, params, "recorder.record", RecorderRun::record)
            .await
    }

    pub(super) async fn recorder_cut(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        self.recorder_verb(conn, params, "recorder.cut", RecorderRun::cut)
            .await
    }

    /// `recorder.stop` and `recorder.abort`: open to every connection.
    pub(super) async fn recorder_end(
        &self,
        conn: &Conn,
        abort: bool,
    ) -> Result<Value, MethodError> {
        let (run, by) = {
            let inner = self.inner.lock().await;
            let Some(run) = inner.recorder.clone() else {
                return Ok(view::state_json(None));
            };
            (run, inner.session.client_name(conn.id, &conn.peer))
        };
        if !run.view().state.is_live() {
            return Ok(view::state_json(Some(&run.view())));
        }
        stop_recorder(&run, abort, "operator", by);
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn recorder_recordings(
        &self,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let since = since_param(params)?;
        let run = self.inner.lock().await.recorder.clone();
        let (list, next) = match run {
            None => (Vec::new(), 0),
            Some(run) => blocking(VERB_WAIT, move || run.recordings(since)).await?,
        };
        Ok(json!({"recordings": list, "next": next}))
    }

    pub(super) async fn recorder_marks(
        &self,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let since = since_param(params)?;
        let run = self.inner.lock().await.recorder.clone();
        let (list, next) = match run {
            None => (Vec::new(), 0),
            Some(run) => blocking(VERB_WAIT, move || run.marks(since)).await?,
        };
        Ok(json!({"marks": list, "next": next}))
    }

    /// `recorder.reset`: configure the present run's configuration again.
    pub(super) async fn recorder_reset(&self, conn: &Conn) -> Result<Value, MethodError> {
        let raw = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.recorder.clone().ok_or_else(not_configured)?;
            inner.run_active()?;
            run.config.clone()
        };
        let (w, report, derived, tbl, station, kind) = self.recorder_check(&raw).await?;
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.run_active()?;
        let Some(derived) = derived else {
            return Err(config::config_invalid(&report));
        };
        let run = self.new_recorder_run(&mut inner, w, derived, &tbl, station, kind);
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn recorder_clear(&self, conn: &Conn) -> Result<Value, MethodError> {
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        if inner.recorder_live() {
            inner.run_active()?;
        }
        inner.recorder = None;
        let state = view::state_json(None);
        inner.recorder_state_key = view::state_key(None);
        self.emit("recorder.state", state.clone());
        Ok(state)
    }

    pub(super) async fn recorder_state_now(&self) -> Value {
        let inner = self.inner.lock().await;
        let v = inner.recorder.as_ref().map(|r| r.view());
        view::state_json(v.as_deref())
    }

    pub(super) async fn recorder_log(
        &self,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let since = since_param(params)?;
        let inner = self.inner.lock().await;
        let (entries, next) = match &inner.recorder {
            Some(r) => r.shared.log_since(since),
            None => (Vec::new(), 0),
        };
        Ok(json!({
            "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
            "next": next,
        }))
    }

    /// `recorder.subscribe`: replace `conn`'s subscription, and send it the run's frame
    /// at once when the run has started.
    pub(super) async fn recorder_subscribe(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let sub = subscribe::parse(params)?.map(Arc::new);
        let result = subscribe::result_json(sub.as_deref());
        match &sub {
            None => {
                self.recorder_displays().remove(&conn.id);
            }
            Some(sub) => {
                self.recorder_displays().insert(
                    conn.id,
                    RecorderDisplay {
                        sub: sub.clone(),
                        last: None,
                    },
                );
                let run = self.inner.lock().await.recorder.clone();
                if let Some(run) = run.filter(|r| r.started()) {
                    self.emit_recorder_data(conn.id, &run, sub);
                    if let Some(d) = self.recorder_displays().get_mut(&conn.id) {
                        d.last = Some(Instant::now());
                    }
                }
            }
        }
        Ok(result)
    }

    fn emit_recorder_data(&self, conn: u64, run: &RecorderRun, sub: &Subscription) {
        let v = run.view();
        let (first, cols) = run.data.time();
        let mut arrays = Arrays::new();
        let data = run.data.with_spectra(|spectra, frf| {
            let ctx = DataContext {
                view: &v,
                time: Some((first, &cols)),
                spectra,
                frf,
            };
            subscribe::data_json(&ctx, sub, &mut arrays)
        });
        let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match codec::encode(&protocol::event("recorder.data", seq, data), &arrays) {
            Ok(frame) => {
                let _ = self
                    .events
                    .send(Arc::new(Out::To(conn, Feed::Recorder, frame)));
            }
            Err(e) => eprintln!("sds-control: recorder.data frame not encoded: {e}"),
        }
    }

    /// Each subscribed connection whose frame is due (every one when `force`).
    fn recorder_data_due(&self, run: &RecorderRun, force: bool) {
        let now = Instant::now();
        let due: Vec<(u64, Arc<Subscription>)> = {
            let mut displays = self.recorder_displays();
            displays
                .iter_mut()
                .filter(|(_, d)| {
                    force
                        || d.last
                            .is_none_or(|t| now + DATA_SLACK >= t + d.sub.period())
                })
                .map(|(id, d)| {
                    d.last = Some(now);
                    (*id, d.sub.clone())
                })
                .collect()
        };
        for (conn, sub) in due {
            self.emit_recorder_data(conn, run, &sub);
        }
    }

    /// A run published `v`: `recorder.state` when what it shows changed, and its new
    /// log entries, to every connection.
    pub(super) async fn recorder_changed(&self, run_id: &str, v: &RecorderView) {
        let mut inner = self.inner.lock().await;
        let Some(run) = inner.recorder.clone() else {
            return;
        };
        if run.id != run_id {
            return;
        }
        let key = view::state_key(Some(v));
        if key != inner.recorder_state_key {
            inner.recorder_state_key = key;
            self.emit("recorder.state", view::state_json(Some(v)));
        }
        let since = if inner.recorder_log_sent.0 == run.id {
            inner.recorder_log_sent.1
        } else {
            0
        };
        let (entries, next) = run.shared.log_since(since);
        inner.recorder_log_sent = (run.id.clone(), next);
        if !entries.is_empty() {
            self.emit(
                "recorder.log",
                json!({
                    "run_id": run.id,
                    "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
                }),
            );
        }
    }

    /// Publish `run`'s `recorder.live` ten times a second from start until it has
    /// finished, then once more, to the connections without a subscription of their
    /// own; and `recorder.data` to the subscribed ones at their rate.
    fn spawn_recorder_live(self: &Arc<Self>, run: Arc<RecorderRun>) {
        if run
            .shared
            .live_started
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let svc = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(LIVE_TICK);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last_live: Option<Instant> = None;
            loop {
                tick.tick().await;
                let current = svc
                    .inner
                    .lock()
                    .await
                    .recorder
                    .as_ref()
                    .is_some_and(|r| Arc::ptr_eq(r, &run));
                let v = run.view();
                let last = v.state == State::Finished || !current;
                let now = Instant::now();
                if last || last_live.is_none_or(|t| now + DATA_SLACK >= t + LIVE_PERIOD) {
                    last_live = Some(now);
                    let mut arrays = Arrays::new();
                    let data = view::live_json(&v, &mut arrays);
                    let seq = svc.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    match codec::encode(&protocol::event("recorder.live", seq, data), &arrays) {
                        Ok(frame) => {
                            let _ = svc
                                .events
                                .send(Arc::new(Out::DefaultLive(Feed::Recorder, frame)));
                        }
                        Err(e) => eprintln!("sds-control: recorder.live frame not encoded: {e}"),
                    }
                }
                if current {
                    svc.recorder_data_due(&run, last);
                }
                if last {
                    break;
                }
            }
        });
    }
}

/// End `run` for `by`: the stop (`abort` false, for `cause`) or the abort. Both end
/// acquisition at the next batch and finalize every recording; it answers at once.
pub(super) fn stop_recorder(run: &RecorderRun, abort: bool, cause: &'static str, by: String) {
    run.shared.set_ended_by(&by);
    run.request_stop(abort, cause);
}
