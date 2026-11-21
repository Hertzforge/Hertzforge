//! The `modal_test.*` methods (schema/PROTOCOL.md, "Modal Test"): configure with its
//! report, start, the operator's calls (select, next, previous, arm, disarm, accept,
//! reject, undo, the target from the latest hit, drive start and stop in shaker mode),
//! end, abort, reset, clear, the state and the log, the simulator's hits, and the run's
//! `modal_test.state`, `modal_test.log` and `modal_test.live` events.

use super::{Conn, Out, Service, DATA_SLACK, LIVE_TICK};
use crate::codec::{self, Arrays};
use crate::modal_test::config::{self, WireModal};
use crate::modal_test::run::{self as mrun, Cmd, ModalRun, ModalView};
use crate::playback::config::config_invalid;
use crate::protocol::{self, ErrorCode, MethodError};
use crate::pulse::rails::Shared;
use crate::station::{self, Selection, StationPlan};
use crate::table::{self, Item, Report};
use hertzforge_channel_table::ChannelTable;
use hertzforge_modal_test::host::capture_only_table;
use hertzforge_modal_test::plan::Plan;
use hertzforge_modal_test::session::Session;
use hertzforge_modal_test::setup::{Mode, STOP_RAMP_S};
use hertzforge_modal_test::structure::HitKind;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often `modal_test.live` goes out.
const LIVE_PERIOD: Duration = Duration::from_millis(100);

fn not_configured() -> MethodError {
    MethodError::named(
        ErrorCode::Refused,
        "not_configured",
        "The Modal Test is not configured; configure a test first.",
    )
}

/// How long the shutdown waits for the controlled end before it aborts.
pub(super) fn wait_for(_run: &ModalRun) -> Duration {
    Duration::from_secs_f64((2.0 * STOP_RAMP_S).max(10.0))
}

fn since_param(params: &Map<String, Value>) -> Result<Option<u64>, MethodError> {
    match params.get("since") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| MethodError::bad_params("since", "\"since\" is a count")),
    }
}

/// The table a test's station is opened from: in impact mode without its drive rows,
/// so no output is opened and no drive sample is written.
pub fn station_table(mode: Mode, table: &ChannelTable) -> ChannelTable {
    match mode {
        Mode::Impact => capture_only_table(table),
        Mode::Shaker => table.clone(),
    }
}

/// The simulator a test arms on: the station's own, widened to the shakers. The
/// structure is the service's ([`crate::modal_test::sim_source`]).
pub fn sim_plan(fs_hz: f64, mode: Mode, table: &ChannelTable) -> StationPlan {
    let drives = crate::random::record::drive_rows(&station_table(mode, table)).len();
    StationPlan::Sim {
        fs_hz,
        plant: None,
        outputs: (drives > 1).then_some(drives as u32),
    }
}

/// What a configure found.
struct Checked {
    w: WireModal,
    report: Report,
    plan: Option<Plan>,
    check: Value,
    source: &'static str,
    class: &'static str,
}

impl Service {
    async fn modal_check(&self, config: &Value) -> Result<Checked, MethodError> {
        let w = config::parse(config)?;
        let (tbl, mut report, source, class) = {
            let inner = self.inner.lock().await;
            let tbl = inner.table.table.clone();
            let report = table::validate(&tbl, &inner.ctx());
            let sim = *inner.stations.selected() == Selection::Sim;
            (
                tbl,
                report,
                if sim { "SIM" } else { "HARDWARE" },
                station::class_name(inner.stations.class()),
            )
        };
        let (r, plan) = config::check(&w, &tbl);
        report.errors.extend(r.errors);
        let check = match &plan {
            Some(p) => {
                report.warnings.extend(config::warnings(&w, p));
                config::report(&w, p, source)
            }
            None => Value::Null,
        };
        Ok(Checked {
            w,
            report,
            plan,
            check,
            source,
            class,
        })
    }

    pub(super) async fn modal_test_configure(
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
        let c = self.modal_check(config).await?;
        let warnings: Vec<Value> = c.report.warnings.iter().map(Item::to_json).collect();
        if dry_run {
            return Ok(json!({
                "errors": c.report.errors.iter().map(Item::to_json).collect::<Vec<_>>(),
                "warnings": warnings,
                "check": c.check,
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
        if !c.report.errors.is_empty() || c.plan.is_none() {
            return Err(config_invalid(&c.report, Some(&c.check)));
        }
        let run = self.new_modal_run(&mut inner, c);
        Ok(json!({
            "state": mrun::state_json(Some(&run.view())),
            "check": run.report,
            "warnings": warnings,
        }))
    }

    fn new_modal_run(&self, inner: &mut super::Inner, c: Checked) -> Arc<ModalRun> {
        inner.modal_test_runs += 1;
        let run_id = format!("modal-test-{:04}", inner.modal_test_runs);
        let plan = c.plan.expect("a configure with no errors has a plan");
        let session = Session::new(c.w.setup.clone(), plan.clone());
        let initial = mrun::configured_view(&run_id, &c.w, &session, c.source, c.class);
        let shared = Arc::new(Shared::new(initial.clone(), self.modal_test_notify.clone()));
        let title = c.w.title.clone();
        let mode = c.w.setup.mode.name();
        let run = Arc::new(ModalRun::new(
            run_id.clone(),
            shared.clone(),
            c.w,
            plan,
            session,
            c.check,
        ));
        shared.add_log(
            "state",
            format!(
                "configured \"{title}\" ({mode} mode) on {}",
                initial.station_class
            ),
        );
        inner.modal_test = Some(run.clone());
        let _ = self.modal_test_notify.send((run_id, Arc::new(initial)));
        run
    }

    pub(super) async fn modal_test_config(&self) -> Value {
        let inner = self.inner.lock().await;
        match &inner.modal_test {
            None => json!({"run_id": null, "config": null, "check": null}),
            Some(r) => json!({"run_id": r.id, "config": r.config, "check": r.report}),
        }
    }

    pub(super) async fn modal_test_start(
        self: &Arc<Self>,
        conn: &Conn,
    ) -> Result<Value, MethodError> {
        let (run, ctx) = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.modal_test.clone().ok_or_else(not_configured)?;
            let v = run.view();
            if v.state == "finished" {
                return Err(MethodError::named(
                    ErrorCode::Refused,
                    "run_finished",
                    "This test has finished; reset or configure again to run.",
                )
                .with_detail(json!({"outcome": v.end.as_ref().map(|e| e.outcome)})));
            }
            if v.is_live() || run.started() {
                return Err(mrun::refuse(
                    "wrong_state",
                    format!(
                        "modal_test.start does not apply while the test is {}.",
                        v.state
                    ),
                ));
            }
            inner.run_active()?;
            if inner.sim_running() {
                return Err(MethodError::named(
                    ErrorCode::Refused,
                    "run_active",
                    "A simulation is running; stop it first.",
                ));
            }
            inner.table.arm_gate()?;
            let tbl = inner.table.table.clone();
            let report = table::validate(&tbl, &inner.ctx());
            if !report.errors.is_empty() {
                return Err(super::table_invalid(
                    &report,
                    "on this station, so the test cannot start",
                ));
            }
            // The table as it is now must still carry the test as it was configured.
            let (mut r, plan) = config::check(&run.wire, &tbl);
            if plan.as_ref().is_some_and(|p| *p != run.plan) {
                r.errors.push(Item::field(
                    "table_changed",
                    "config",
                    "The channel table's rows have changed since the test was configured; \
                     configure it again.",
                ));
            }
            if !r.errors.is_empty() {
                return Err(config_invalid(&r, None));
            }
            let s = &run.wire.setup;
            let fs = s.fs_hz;
            let plan = match inner.stations.selected() {
                Selection::Sim => sim_plan(fs, s.mode, &tbl),
                Selection::Ni { devices } => {
                    let (inputs, outputs) =
                        hertzforge_station::physical_from_table(&station_table(s.mode, &tbl));
                    StationPlan::Ni {
                        devices: devices.clone(),
                        inputs,
                        outputs,
                        fs_hz: fs,
                    }
                }
            };
            run.mark_starting();
            let ctx = mrun::StartContext {
                table: tbl,
                station: plan,
                scanner: self.opts.scanner.clone(),
                data_root: self.opts.data_root.clone(),
                records_root: self.opts.records_root(),
                bind: self.opts.bind.clone(),
                revision: inner.table.revision,
                devices: inner.stations.devices().to_vec(),
                record: self.opts.record.clone(),
                sim_speed: self.opts.sim_speed,
            };
            (run, ctx)
        };
        let r = run.clone();
        let started = tokio::task::spawn_blocking(move || r.start(ctx))
            .await
            .map_err(|e| MethodError::internal(format!("the start failed: {e}")))?;
        self.spawn_modal_test_live(run.clone());
        started?;
        Ok(mrun::state_json(Some(&run.view())))
    }

    /// An operator's call to the run's thread: the controller's.
    pub(super) async fn modal_test_call(
        &self,
        conn: &Conn,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let run = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            inner.modal_test.clone().ok_or_else(not_configured)?
        };
        let cmd = match method {
            "select" => {
                let p = params.get("point").and_then(Value::as_u64).ok_or_else(|| {
                    MethodError::bad_params("point", "Selecting needs a point's index.")
                })?;
                Cmd::Select(p as usize)
            }
            "next" => Cmd::Step(1),
            "previous" => Cmd::Step(-1),
            "arm" => Cmd::Arm,
            "disarm" => Cmd::Disarm,
            "accept" => Cmd::Accept,
            "reject" => Cmd::Reject,
            "undo" => Cmd::Undo,
            "target_from_hit" => Cmd::TargetFromHit,
            "drive_start" => Cmd::DriveStart,
            "drive_stop" => Cmd::DriveStop,
            "sim_hit" => {
                let k = params
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("clean");
                Cmd::SimHit(HitKind::from_name(k).ok_or_else(|| {
                    MethodError::bad_params("kind", "A simulated hit is clean, double or overload.")
                })?)
            }
            _ => return Err(MethodError::internal("no such Modal Test call")),
        };
        if !run.view().is_live() {
            return Err(mrun::refuse(
                "not_live",
                format!("modal_test.{method} needs a started test."),
            ));
        }
        let r = tokio::task::spawn_blocking(move || run.call(cmd))
            .await
            .map_err(|e| MethodError::internal(format!("the call failed: {e}")))?;
        let run = self.inner.lock().await.modal_test.clone();
        let mut out = r?;
        if let (Some(o), Some(run)) = (out.as_object_mut(), run) {
            o.insert("state".into(), mrun::state_json(Some(&run.view())));
        }
        Ok(out)
    }

    /// `modal_test.end` and `modal_test.abort`: open to every connection.
    pub(super) async fn modal_test_end(
        &self,
        conn: &Conn,
        abort: bool,
    ) -> Result<Value, MethodError> {
        let (run, by) = {
            let inner = self.inner.lock().await;
            let Some(run) = inner.modal_test.clone() else {
                return Ok(mrun::state_json(None));
            };
            (run, inner.session.client_name(conn.id, &conn.peer))
        };
        if !run.view().is_live() {
            return Ok(mrun::state_json(Some(&run.view())));
        }
        if abort {
            run.abort(&by);
        } else {
            run.end(&by);
        }
        Ok(mrun::state_json(Some(&run.view())))
    }

    pub(super) async fn modal_test_reset(&self, conn: &Conn) -> Result<Value, MethodError> {
        let raw = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.modal_test.clone().ok_or_else(not_configured)?;
            inner.run_active()?;
            run.config.clone()
        };
        let mut params = Map::new();
        params.insert("config".into(), raw);
        let r = self.modal_test_configure(conn, &params).await?;
        Ok(r["state"].clone())
    }

    pub(super) async fn modal_test_clear(&self, conn: &Conn) -> Result<Value, MethodError> {
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        if inner.modal_test_live() {
            inner.run_active()?;
        }
        inner.modal_test = None;
        let state = mrun::state_json(None);
        inner.modal_test_state_key = mrun::state_key(None);
        self.emit("modal_test.state", state.clone());
        Ok(state)
    }

    pub(super) async fn modal_test_state_now(&self) -> Value {
        let inner = self.inner.lock().await;
        let v = inner.modal_test.as_ref().map(|r| r.view());
        mrun::state_json(v.as_deref())
    }

    pub(super) async fn modal_test_log(
        &self,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let since = since_param(params)?.unwrap_or(0);
        let inner = self.inner.lock().await;
        let (entries, next) = match &inner.modal_test {
            Some(r) => r.shared.log_since(since),
            None => (Vec::new(), 0),
        };
        Ok(json!({
            "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
            "next": next,
        }))
    }

    /// A test published `v`: `modal_test.state` when what it shows changed, and its new
    /// log entries.
    pub(super) async fn modal_test_changed(&self, run_id: &str, v: &ModalView) {
        let mut inner = self.inner.lock().await;
        let Some(run) = inner.modal_test.clone() else {
            return;
        };
        if run.id != run_id {
            return;
        }
        let key = mrun::state_key(Some(v));
        if key != inner.modal_test_state_key {
            inner.modal_test_state_key = key;
            self.emit("modal_test.state", mrun::state_json(Some(v)));
        }
        let since = if inner.modal_test_log_sent.0 == run.id {
            inner.modal_test_log_sent.1
        } else {
            0
        };
        let (entries, next) = run.shared.log_since(since);
        inner.modal_test_log_sent = (run.id.clone(), next);
        if !entries.is_empty() {
            self.emit(
                "modal_test.log",
                json!({
                    "run_id": run.id,
                    "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
                }),
            );
        }
    }

    /// Publish `run`'s `modal_test.live` ten times a second from start until it has
    /// finished, then once more, to every connection.
    fn spawn_modal_test_live(self: &Arc<Self>, run: Arc<ModalRun>) {
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
                    .modal_test
                    .as_ref()
                    .is_some_and(|r| Arc::ptr_eq(r, &run));
                let v = run.view();
                let last = v.state == "finished" || !current;
                let now = Instant::now();
                if last || last_live.is_none_or(|t| now + DATA_SLACK >= t + LIVE_PERIOD) {
                    last_live = Some(now);
                    let mut arrays = Arrays::new();
                    let data = {
                        let d = match run.display.lock() {
                            Ok(d) => d,
                            Err(p) => p.into_inner(),
                        };
                        mrun::live_json(&v, &d, &mut arrays)
                    };
                    let seq = svc.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    match codec::encode(&protocol::event("modal_test.live", seq, data), &arrays) {
                        Ok(frame) => {
                            let _ = svc.events.send(Arc::new(Out::Binary(frame)));
                        }
                        Err(e) => {
                            eprintln!("sds-control: modal_test.live frame not encoded: {e}")
                        }
                    }
                }
                if last {
                    break;
                }
            }
        });
    }
}

/// Begin `run`'s controlled end, named for `by`.
pub(super) fn end_modal_test(run: &ModalRun, by: String) {
    run.end(&by);
}

/// Abort `run` for `by`.
pub(super) fn abort_modal_test(run: &ModalRun, by: String) {
    run.abort(&by);
}
