//! The MIMO Shock methods of the [`Service`] (schema/PROTOCOL.md, "MIMO Shock"):
//! a configuration and its run beside the other runs, under the same run
//! exclusion and the same controller rules, on the transient-run rails.

use super::{Conn, Feed, Out, Service, DATA_SLACK, DISPLAY_PERIOD, LIVE_TICK};
use crate::codec::{self, Arrays};
use crate::mimo_shock::config::{self, CheckContext, WireMs};
use crate::mimo_shock::run::{self as mrun, ArmContext, Cmd, RunPlan, SIM_STATED_RATE_HZ};
use crate::mimo_shock::subscribe::{self, DataContext, Subscription};
use crate::mimo_shock::view::{self, MsView};
use crate::mimo_shock::{MsRun, RunData};
use crate::protocol::{self, ErrorCode, MethodError};
use crate::pulse::rails::{Jobs, Shared};
use crate::random::view::State;
use crate::shock::view::StationRef;
use crate::station::{self, Selection, StationPlan};
use crate::table;
use hertzforge_mimo_shock::{Target, TargetKind};
use hertzforge_random_laws::LawWorld;
use hertzforge_shock_common::ShockRunMode;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One connection's MIMO Shock subscription and when it was last sent a frame.
pub struct MsDisplay {
    sub: Arc<Subscription>,
    last: Option<Instant>,
}

/// How long to wait for the run thread to answer: its identification, a few
/// records, and a margin.
fn wait_for(run: &MsRun) -> Duration {
    let record_s = run.derived_full.record_len as f64 / run.derived_full.fs_hz.max(1.0);
    let frames = run.wire.frames() as f64;
    Duration::from_secs_f64(30.0 + (2.0 * frames + 20.0) * record_s)
}

const ARM_WAIT: Duration = Duration::from_secs(120);

fn not_configured() -> MethodError {
    MethodError::named(
        ErrorCode::Refused,
        "not_configured",
        "MIMO Shock is not configured; configure a test first.",
    )
}

impl Service {
    pub(super) fn mimo_shock_displays(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<u64, MsDisplay>> {
        match self.mimo_shock_displays.lock() {
            Ok(d) => d,
            Err(p) => p.into_inner(),
        }
    }

    fn ms_station_ref(inner: &super::Inner) -> StationRef {
        let selected = inner.stations.selected().clone();
        StationRef {
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
        }
    }

    /// Parse and check `config` against the present table.
    async fn mimo_shock_check(
        &self,
        config: &Value,
    ) -> Result<
        (
            WireMs,
            table::Report,
            Option<config::Derived>,
            Option<config::LawFacts>,
        ),
        MethodError,
    > {
        let w = config::parse(config)?;
        let (tbl, table_report) = {
            let inner = self.inner.lock().await;
            (
                inner.table.table.clone(),
                table::validate(&inner.table.table, &inner.ctx()),
            )
        };
        let timeout_s = self.opts.timeout.as_secs_f64();
        let id = w.law_id.clone();
        let (w, report, derived, facts) = self
            .with_catalog(move |cat| {
                let lookup = |i: &str| crate::mimo_shock::laws::facts(cat, i);
                let ctx = CheckContext {
                    table: &tbl,
                    law: &lookup,
                    timeout_s,
                };
                let (report, derived) = w.check(&ctx);
                let facts = crate::mimo_shock::laws::facts(cat, &id);
                Ok((w, report, derived, facts))
            })
            .await?;
        let mut all = table_report;
        all.extend(report);
        let derived = if all.errors.is_empty() { derived } else { None };
        Ok((w, all, derived, facts))
    }

    pub(super) async fn mimo_shock_configure(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<(Value, Arrays), MethodError> {
        let config = params
            .get("config")
            .filter(|c| c.is_object())
            .ok_or_else(|| {
                MethodError::bad_params("config", "Configuring needs a test configuration.")
            })?;
        let dry_run = protocol::param_bool(params, "dry_run")?.unwrap_or(false);
        let (w, report, derived, facts) = self.mimo_shock_check(config).await?;
        if dry_run {
            let mut arrays = Arrays::new();
            let views = match &derived {
                Some(d) => Value::from(target_views(&w, d, &mut arrays)),
                None => json!([]),
            };
            return Ok((
                json!({
                    "errors": report.errors.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
                    "warnings": report.warnings.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
                    "derived": derived.as_ref().map_or(Value::Null, |d| d.to_json()),
                    "views": views,
                }),
                arrays,
            ));
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
        let (Some(derived), Some(facts)) = (derived, facts) else {
            return Err(config::config_invalid(&report));
        };
        let run = self.new_mimo_shock_run(&mut inner, w, derived, &facts)?;
        let v = run.view();
        Ok((
            json!({
                "state": view::state_json(Some(&v)),
                "derived": run.derived,
                "warnings": report.warnings.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
            }),
            Arrays::new(),
        ))
    }

    fn new_mimo_shock_run(
        &self,
        inner: &mut super::Inner,
        w: WireMs,
        derived: config::Derived,
        facts: &config::LawFacts,
    ) -> Result<Arc<MsRun>, MethodError> {
        inner.mimo_shock_runs += 1;
        let run_id = format!("ms-{:04}", inner.mimo_shock_runs);
        let station = Self::ms_station_ref(inner);
        let initial = crate::mimo_shock::configured_view(&run_id, &w, &derived, station, facts);
        let shared = Arc::new(Shared::new(initial.clone(), self.mimo_shock_notify.clone()));
        let data = Arc::new(RunData::default());
        let raw = w.raw.clone();
        let record_len = derived.record_len;
        let wire = Arc::new(w);
        let plan = RunPlan {
            run_id: run_id.clone(),
            cfg: wire.clone(),
            record_len,
            timeout_s: self.opts.timeout.as_secs_f64(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        mrun::spawn(plan, initial.clone(), shared.clone(), data.clone(), rx)
            .map_err(|e| MethodError::internal(format!("could not start the run thread: {e}")))?;
        let run = Arc::new(MsRun {
            id: run_id.clone(),
            shared: shared.clone(),
            data,
            config: raw,
            derived: derived.to_json(),
            derived_full: Arc::new(derived),
            wire: wire.clone(),
            jobs: Jobs::new(&run_id, tx),
        });
        inner.mimo_shock = Some(run.clone());
        inner.mimo_shock_rows_sent = 0;
        inner.mimo_shock_ident_sent = false;
        shared.add_log(
            "state",
            format!("configured \"{}\" on {}", wire.title, initial.station.class),
        );
        let _ = self.mimo_shock_notify.send((run_id, Arc::new(initial)));
        Ok(run)
    }

    pub(super) async fn mimo_shock_config(&self) -> (Value, Arrays) {
        let inner = self.inner.lock().await;
        let mut arrays = Arrays::new();
        let result = match &inner.mimo_shock {
            None => json!({"run_id": null, "config": null, "derived": null, "views": []}),
            Some(run) => json!({
                "run_id": run.id,
                "config": run.config,
                "derived": run.derived,
                "views": target_views(&run.wire, &run.derived_full, &mut arrays),
            }),
        };
        (result, arrays)
    }

    async fn controlled_mimo_shock(&self, conn: &Conn) -> Result<Arc<MsRun>, MethodError> {
        let inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.mimo_shock.clone().ok_or_else(not_configured)
    }

    /// What a run arms on now: the table, the selected station and its law.
    async fn mimo_shock_arm_context(&self, run: &MsRun) -> Result<ArmContext, MethodError> {
        let w = run.wire.clone();
        let (tbl, station_plan, hardware) = {
            let inner = self.inner.lock().await;
            let report = table::validate(&inner.table.table, &inner.ctx());
            if !report.errors.is_empty() {
                return Err(super::table_invalid(
                    &report,
                    "on this station, so the run cannot arm",
                ));
            }
            let (plan, hardware) = match inner.stations.selected() {
                Selection::Sim => (sim_plan(&w, &inner.table.table)?, false),
                Selection::Ni { devices } => {
                    let (inputs, outputs) =
                        hertzforge_station::physical_from_table(&inner.table.table);
                    (
                        StationPlan::Ni {
                            devices: devices.clone(),
                            inputs,
                            outputs,
                            fs_hz: w.fs_hz,
                        },
                        station::class_name(inner.stations.class()) == "ni_real",
                    )
                }
            };
            (inner.table.table.clone(), plan, hardware)
        };
        let id = w.law_id.clone();
        let given = w.law_params.clone();
        let budget = Duration::from_secs_f64((w.law_budget * w.spacing_s()).max(1e-3));
        let seed_budget = Duration::from_secs_f64(w.seed_budget_s);
        let chosen = self
            .with_catalog(move |cat| {
                Ok(crate::mimo_shock::laws::resolve(
                    cat,
                    &id,
                    &given,
                    budget,
                    seed_budget,
                    hardware,
                ))
            })
            .await?;
        let (law, host_rule, law_values) = match chosen {
            Ok(c) => (Ok(c.factory), c.host_rule, c.values),
            Err(e) => (Err(e), None, Vec::new()),
        };
        Ok(ArmContext {
            table: tbl,
            station: station_plan,
            scanner: self.opts.scanner.clone(),
            data_root: self.opts.data_root.clone(),
            capture_root: self.opts.capture_root.clone(),
            bind: self.opts.bind.clone(),
            law,
            host_rule,
            law_values,
        })
    }

    pub(super) async fn mimo_shock_arm(
        self: &Arc<Self>,
        conn: &Conn,
        start: bool,
    ) -> Result<Value, MethodError> {
        let run = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.mimo_shock.clone().ok_or_else(not_configured)?;
            let v = run.view();
            if v.state == State::Finished {
                return Err(MethodError::named(
                    ErrorCode::Refused,
                    "run_finished",
                    "This run has finished; reset or configure again to run.",
                )
                .with_detail(json!({"outcome": v.end.as_ref().map(|e| e.outcome)})));
            }
            if inner.random_live()
                || inner.mimo_live()
                || inner.shock_live()
                || inner.srs_live()
                || inner.sine_live()
                || inner.mixed_live()
                || inner.twr_live()
                || inner.explorer_live()
                || inner.recorder_live()
            {
                inner.run_active()?;
            }
            if v.state.is_busy() || (!start && v.state == State::Armed) {
                inner.run_active()?;
            }
            if inner.sim_running() {
                return Err(MethodError::named(
                    ErrorCode::Refused,
                    "run_active",
                    "A simulation is running; stop it first.",
                ));
            }
            run
        };
        let ctx = self.mimo_shock_arm_context(&run).await?;
        if start {
            run.jobs
                .ask(|reply| Cmd::Start { ctx, reply }, ARM_WAIT)
                .await?;
        } else {
            run.jobs
                .ask(|reply| Cmd::Arm { ctx, reply }, ARM_WAIT)
                .await?;
        }
        self.spawn_mimo_shock_live(run.clone());
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn mimo_shock_proceed(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let ack = match params.get("acknowledge") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_owned()),
            Some(_) => {
                return Err(MethodError::bad_params(
                    "acknowledge",
                    "An acknowledgement is the operator's own words; it cannot be empty.",
                ))
            }
        };
        let run = self.controlled_mimo_shock(conn).await?;
        let v = run.view();
        if v.state != State::Review {
            return Err(mrun::wrong_state(&v, "Proceeding"));
        }
        run.jobs
            .ask(|reply| Cmd::Proceed { ack, reply }, wait_for(&run))
            .await?;
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn mimo_shock_fire(&self, conn: &Conn) -> Result<Value, MethodError> {
        let run = self.controlled_mimo_shock(conn).await?;
        if run.wire.mode == ShockRunMode::Auto {
            return Err(mrun::wrong_mode("A fire"));
        }
        let v = run.view();
        if v.phase == "firing" {
            return Err(mrun::not_yet(
                run.derived_full.record_len as f64 / run.derived_full.fs_hz.max(1.0),
            ));
        }
        if !(v.state == State::Running && v.phase == "waiting") {
            return Err(mrun::wrong_state(&v, "A fire"));
        }
        run.jobs
            .ask(|reply| Cmd::Fire { reply }, wait_for(&run))
            .await?;
        Ok(view::state_json(Some(&run.view())))
    }

    /// `mimo_shock.stop` and `mimo_shock.abort`: open to every connection.
    pub(super) async fn mimo_shock_end(
        &self,
        conn: &Conn,
        abort: bool,
    ) -> Result<Value, MethodError> {
        let (run, by) = {
            let inner = self.inner.lock().await;
            let Some(run) = inner.mimo_shock.clone() else {
                return Ok(view::state_json(None));
            };
            (run, inner.session.client_name(conn.id, &conn.peer))
        };
        if !run.view().state.is_live() {
            return Ok(view::state_json(Some(&run.view())));
        }
        stop_mimo_shock(
            &run,
            if abort { None } else { Some("operator_stop") },
            by,
            true,
        )
        .await;
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn mimo_shock_reset(&self, conn: &Conn) -> Result<Value, MethodError> {
        let (raw, busy) = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.mimo_shock.clone().ok_or_else(not_configured)?;
            (run.config.clone(), run.view().state.is_live())
        };
        if busy {
            self.inner.lock().await.run_active()?;
        }
        let (w, report, derived, facts) = self.mimo_shock_check(&raw).await?;
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.run_active()?;
        let (Some(derived), Some(facts)) = (derived, facts) else {
            return Err(config::config_invalid(&report));
        };
        let run = self.new_mimo_shock_run(&mut inner, w, derived, &facts)?;
        let v = run.view();
        Ok(view::state_json(Some(&v)))
    }

    pub(super) async fn mimo_shock_clear(&self, conn: &Conn) -> Result<Value, MethodError> {
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        if inner.mimo_shock_live() {
            inner.run_active()?;
        }
        inner.mimo_shock = None;
        let state = view::state_json(None);
        inner.mimo_shock_state_key = view::state_key(None);
        self.emit("mimo_shock.state", state.clone());
        Ok(state)
    }

    pub(super) async fn mimo_shock_state_now(&self) -> Value {
        let inner = self.inner.lock().await;
        let v = inner.mimo_shock.as_ref().map(|r| r.view());
        view::state_json(v.as_deref())
    }

    pub(super) async fn mimo_shock_log(
        &self,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let since = since_param(params)?;
        let inner = self.inner.lock().await;
        let (entries, next) = match &inner.mimo_shock {
            Some(r) => r.log_since(since),
            None => (Vec::new(), 0),
        };
        Ok(json!({
            "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
            "next": next,
        }))
    }

    pub(super) async fn mimo_shock_pulses(
        &self,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let since = since_param(params)?;
        let run = self.inner.lock().await.mimo_shock.clone();
        let (rows, next) = match &run {
            Some(r) => r.data.rows_since(since),
            None => (Vec::new(), 0),
        };
        Ok(json!({"pulses": rows, "next": next}))
    }

    pub(super) async fn mimo_shock_pulse(
        &self,
        params: &Map<String, Value>,
    ) -> Result<(Value, Arrays), MethodError> {
        let index = params
            .get("index")
            .and_then(Value::as_u64)
            .ok_or_else(|| MethodError::bad_params("index", "\"index\" is a pulse, from 0"))?;
        let points = crate::shock::subscribe::parse_points(true, params.get("points"), "points")?;
        let run = self
            .inner
            .lock()
            .await
            .mimo_shock
            .clone()
            .ok_or_else(not_configured)?;
        let row = run.data.row(index).ok_or_else(|| {
            MethodError::not_found(format!(
                "No pulse {index} has fired in run {} ({} so far).",
                run.id,
                run.data.row_count()
            ))
        })?;
        let fig = run.data.figures(index).ok_or_else(|| {
            MethodError::not_found(format!(
                "The views of pulse {index} are no longer held; the last {} pulses' are. Its \
                 row is in mimo_shock.pulses.",
                crate::pulse::ring::DEFAULT_CAPACITY
            ))
        })?;
        let mut arrays = Arrays::new();
        let views = view::pulse_views(&fig, points, &mut arrays);
        Ok((json!({"pulse": row, "views": views}), arrays))
    }

    pub(super) async fn mimo_shock_subscribe(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let sub = subscribe::parse(params)?.map(Arc::new);
        let result = subscribe::result_json(sub.as_deref());
        match &sub {
            None => {
                self.mimo_shock_displays().remove(&conn.id);
            }
            Some(sub) => {
                self.mimo_shock_displays().insert(
                    conn.id,
                    MsDisplay {
                        sub: sub.clone(),
                        last: None,
                    },
                );
                let run = self.inner.lock().await.mimo_shock.clone();
                if let Some(run) = run.filter(|r| r.view().streaming) {
                    self.emit_mimo_shock_data(conn.id, &run, sub);
                    if let Some(d) = self.mimo_shock_displays().get_mut(&conn.id) {
                        d.last = Some(Instant::now());
                    }
                }
            }
        }
        Ok(result)
    }

    fn emit_mimo_shock_data(&self, conn: u64, run: &MsRun, sub: &Subscription) {
        let v = run.view();
        let last = run.data.last_figures();
        let ident = run.data.identification();
        let law = run.data.law();
        let (rows, _) = run.data.rows_since(0);
        let ctx = DataContext {
            view: &v,
            last: last.as_deref(),
            identification: ident.as_deref(),
            law: &law,
            rows: &rows,
        };
        let mut arrays = Arrays::new();
        let data = subscribe::data_json(&ctx, sub, &mut arrays);
        let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match codec::encode(&protocol::event("mimo_shock.data", seq, data), &arrays) {
            Ok(frame) => {
                let _ = self
                    .events
                    .send(Arc::new(Out::To(conn, Feed::MimoShock, frame)));
            }
            Err(e) => eprintln!("sds-control: mimo_shock.data frame not encoded: {e}"),
        }
    }

    fn mimo_shock_data_due(&self, run: &MsRun, last: bool) {
        let now = Instant::now();
        let due: Vec<(u64, Arc<Subscription>)> = {
            let mut displays = self.mimo_shock_displays();
            displays
                .iter_mut()
                .filter(|(_, d)| {
                    last || d
                        .last
                        .is_none_or(|t| now + DATA_SLACK >= t + d.sub.period())
                })
                .map(|(id, d)| {
                    d.last = Some(now);
                    (*id, d.sub.clone())
                })
                .collect()
        };
        for (conn, sub) in due {
            self.emit_mimo_shock_data(conn, run, &sub);
        }
    }

    /// A run published `v`: `mimo_shock.state` when what it shows changed, its
    /// new log entries, a `mimo_shock.pulse` for each new pulse, and
    /// `mimo_shock.data` when there is something new to plot.
    pub(super) async fn mimo_shock_changed(&self, run_id: &str, v: &MsView) {
        let mut inner = self.inner.lock().await;
        let Some(run) = inner.mimo_shock.clone() else {
            return;
        };
        if run.id != run_id {
            return;
        }
        let shown = run.view();
        let key = view::state_key(Some(&shown));
        if key != inner.mimo_shock_state_key {
            inner.mimo_shock_state_key = key;
            self.emit("mimo_shock.state", view::state_json(Some(&shown)));
        }
        let since = if inner.mimo_shock_log_sent.0 == run.id {
            inner.mimo_shock_log_sent.1
        } else {
            0
        };
        let (entries, next) = run.log_since(since);
        inner.mimo_shock_log_sent = (run.id.clone(), next);
        if !entries.is_empty() {
            self.emit(
                "mimo_shock.log",
                json!({
                    "run_id": run.id,
                    "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
                }),
            );
        }
        let rows_now = run.data.row_count();
        let first_new = inner.mimo_shock_rows_sent;
        inner.mimo_shock_rows_sent = rows_now;
        let ident_new = v.identification.is_some() && !inner.mimo_shock_ident_sent;
        inner.mimo_shock_ident_sent = v.identification.is_some();
        drop(inner);
        for index in first_new..rows_now {
            let (Some(row), fig) = (run.data.row(index), run.data.figures(index)) else {
                continue;
            };
            let mut arrays = Arrays::new();
            let views = fig.map_or_else(Vec::new, |f| {
                view::pulse_views(&f, Some(subscribe::DEFAULT_POINTS), &mut arrays)
            });
            self.emit_binary(
                "mimo_shock.pulse",
                json!({"run_id": run.id, "pulse": row, "views": views}),
                &arrays,
            );
        }
        let finished = v.state == State::Finished;
        if rows_now > first_new || ident_new || finished {
            self.mimo_shock_data_due(&run, finished);
        }
    }

    /// Publish `run`'s `mimo_shock.live` five times a second until it has
    /// finished, then once more with the final state.
    fn spawn_mimo_shock_live(self: &Arc<Self>, run: Arc<MsRun>) {
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
                    .mimo_shock
                    .as_ref()
                    .is_some_and(|r| Arc::ptr_eq(r, &run));
                let v = run.view();
                if !v.streaming {
                    if !current {
                        break;
                    }
                    continue;
                }
                let last = v.state == State::Finished || !current;
                let now = Instant::now();
                if last || last_live.is_none_or(|t| now >= t + DISPLAY_PERIOD - DATA_SLACK) {
                    last_live = Some(now);
                    svc.emit_binary("mimo_shock.live", view::live_json(&v), &Arrays::new());
                }
                if last {
                    break;
                }
            }
        });
    }

    /// The catalog id of the MIMO Shock law `laws.check` names, by its `id` or
    /// the `path` of its file (imported first, for the controller only), or
    /// `None` when it names no MIMO Shock law.
    pub(super) async fn mimo_shock_law_id(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Option<String> {
        let id = params.get("id").and_then(Value::as_str).map(str::to_owned);
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if path.is_some() {
            let inner = self.inner.lock().await;
            if id.is_some() || inner.session.require_controller(conn.id).is_err() {
                return None;
            }
        }
        self.with_catalog(move |cat| {
            let id = match (id, path) {
                (Some(id), None) => id,
                (None, Some(p)) => crate::laws::import(cat, &p)?["law"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                _ => return Ok(None),
            };
            let builtin = hertzforge_mimo_shock_laws::BuiltinMimoShock::from_id(&id).is_some();
            Ok((builtin
                || cat
                    .entry(&id)
                    .is_some_and(|e| e.world == LawWorld::ShockMimo))
            .then_some(id))
        })
        .await
        .ok()
        .flatten()
    }

    /// `laws.check` of a MIMO Shock law: every case of the check on the
    /// simulator, one `laws.check` event as each starts and as it ends. A law
    /// file's result is recorded against its exact file.
    pub(super) async fn mimo_shock_laws_check(
        self: &Arc<Self>,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        use crate::mimo::law_check::{bad_choice, not_recorded, now_rfc3339, CheckSlot};
        let id = protocol::named_str(params, "id")?
            .map(str::to_owned)
            .ok_or_else(|| {
                MethodError::bad_params("id", "Checking a law needs its catalog \"id\".")
            })?;
        let given = match params.get("params") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(m)) => m.clone(),
            Some(_) => {
                return Err(MethodError::bad_params(
                    "params",
                    "\"params\" is an object of parameter values by key.",
                ))
            }
        };
        {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            inner.run_active()?;
        }
        let law_id = id.clone();
        let (facts, factory) = self
            .with_catalog(move |cat| {
                let facts = crate::mimo_shock::laws::facts(cat, &law_id)
                    .ok_or_else(|| crate::mimo::law_check::unknown_law(&law_id))?;
                for (k, v) in &given {
                    match facts.params.iter().find(|p| &p.key == k) {
                        None => return Err(bad_choice(&law_id, &format!("no parameter {k}"))),
                        Some(spec) => {
                            if let Some(why) = crate::shock::config::param_problem(spec, v) {
                                return Err(bad_choice(&law_id, &format!("{} {why}", spec.label)));
                            }
                        }
                    }
                }
                let chosen = crate::mimo_shock::laws::resolve(
                    cat,
                    &law_id,
                    &given,
                    Duration::from_secs(1),
                    Duration::from_secs_f64(hertzforge_mimo_shock::DEFAULT_SEED_BUDGET_S),
                    false,
                )
                .map_err(|e| bad_choice(&law_id, &e.message))?;
                Ok((facts, chosen.factory))
            })
            .await?;
        let _slot = CheckSlot::take()?;
        let svc = self.clone();
        let check_id = facts.id.clone();
        let verdicts = tokio::task::spawn_blocking(move || {
            crate::mimo_shock::law_check::run_check(&check_id, factory, &mut |event| {
                svc.emit("laws.check", event);
            })
        })
        .await
        .map_err(|e| MethodError::internal(format!("the law check failed: {e}")))??;
        let (law, recorded) = if facts.builtin {
            let id = facts.id.clone();
            let law = self
                .with_catalog(move |cat| {
                    crate::mimo_shock::laws::list(cat)
                        .into_iter()
                        .find(|e| e["id"] == id.as_str())
                        .ok_or_else(|| MethodError::internal("the built-in law left the catalog"))
                })
                .await?;
            (law, false)
        } else {
            let sha = facts.sha256.clone();
            let v2 = verdicts.clone();
            let id = facts.id.clone();
            let law = self
                .with_catalog(move |cat| {
                    let record =
                        hertzforge_mimo_shock_laws::check_record(&sha, &now_rfc3339(), &v2);
                    cat.record_check(&record)
                        .map_err(|e| not_recorded(&e.to_string()))?;
                    crate::mimo_shock::laws::list(cat)
                        .into_iter()
                        .find(|e| e["id"] == id.as_str())
                        .ok_or_else(|| MethodError::internal("the checked law left the catalog"))
                })
                .await?;
            (law, true)
        };
        Ok(json!({
            "law": law,
            "report": crate::mimo_shock::law_check::report_json(&verdicts, recorded),
        }))
    }
}

/// The simulator station for `w` on `table`: the plant by name, the table its
/// size, stated at [`SIM_STATED_RATE_HZ`] (the run asks its own rate of it).
fn sim_plan(
    w: &WireMs,
    table: &hertzforge_channel_table::ChannelTable,
) -> Result<StationPlan, MethodError> {
    use hertzforge_channel_table::ChannelRole;
    let name = w.sim_plant().ok_or_else(|| {
        MethodError::named(
            ErrorCode::Refused,
            "arm_refused",
            "The simulator plays a shock plant of 4 controls by 4 drives or 6 by 3; name one \
             in \"simulator.plant\" or size the run to one.",
        )
    })?;
    let plant = hertzforge_station::SimShockPlant::from_name(name).ok_or_else(|| {
        MethodError::named(
            ErrorCode::Refused,
            "arm_refused",
            format!("No plant \"{name}\"."),
        )
    })?;
    let (n_in, n_d) = plant.dims();
    let inputs = table
        .channels
        .iter()
        .filter(|c| c.enabled && matches!(c.role, ChannelRole::Control | ChannelRole::Monitor))
        .count();
    let drives = table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role == ChannelRole::Drive)
        .count();
    if (inputs, drives) != (n_in, n_d) {
        return Err(MethodError::named(
            ErrorCode::Refused,
            "arm_refused",
            format!(
                "The simulator's plant \"{name}\" has {n_in} inputs and {n_d} drives; the table \
                 has {inputs} enabled inputs and {drives} drives. Enable that many rows."
            ),
        )
        .with_detail(json!({"plant": name, "inputs": n_in, "drives": n_d})));
    }
    Ok(StationPlan::Sim {
        fs_hz: SIM_STATED_RATE_HZ,
        plant: Some(hertzforge_station::SimPlant::Shock(plant)),
        outputs: Some(n_d as u32),
    })
}

/// The configuration's views: each control's target at full level, and for an
/// SRS target its target SRS.
pub fn target_views(w: &WireMs, d: &config::Derived, arrays: &mut Arrays) -> Vec<Value> {
    use crate::views::{self as vw, Axis, Role, Scale, Series};
    let n = d.record_len;
    let t: Vec<f64> = (0..n).map(|i| i as f64 / d.fs_hz).collect();
    let mut out = Vec::new();
    for (c, (ch, _)) in d.controls.iter().enumerate() {
        let r = &d.target.waveforms()[c * n..(c + 1) * n];
        let mut v = vw::spectrum(
            arrays,
            &format!("Target {ch}"),
            &Axis::new("Time", Some("s"), Scale::Linear),
            &Axis::new("Acceleration", Some("g"), Scale::Linear),
            vec![crate::shock::view::time_series(
                "Target",
                Role::Reference,
                t.clone(),
                r,
                Some(subscribe::DEFAULT_POINTS),
            )],
            Some("time"),
        );
        if let Value::Object(m) = &mut v {
            m.insert("channel".into(), Value::from(*ch));
        }
        out.push(v);
        if let Target::Srs(s) = &d.target {
            let nf = s.natural_hz.len();
            let p = &s.points[c * nf..(c + 1) * nf];
            let k = |db: f64| 10f64.powf(db / 20.0);
            let mut v = vw::spectrum(
                arrays,
                &format!("Target SRS {ch}"),
                &Axis::new("Natural frequency", Some("Hz"), Scale::Log),
                &Axis::new("Peak acceleration", Some("g"), Scale::Log),
                vec![
                    Series::new("Target", Role::Reference, s.natural_hz.clone(), p.to_vec()),
                    Series::new(
                        "Tolerance high",
                        Role::Tolerance,
                        s.natural_hz.clone(),
                        p.iter().map(|x| x * k(s.tol_hi_db)).collect(),
                    ),
                    Series::new(
                        "Tolerance low",
                        Role::Tolerance,
                        s.natural_hz.clone(),
                        p.iter().map(|x| x * k(s.tol_lo_db)).collect(),
                    ),
                ],
                Some("frequency"),
            );
            if let Value::Object(m) = &mut v {
                m.insert("channel".into(), Value::from(*ch));
            }
            out.push(v);
        }
    }
    let _ = (w, TargetKind::Srs);
    out
}

fn since_param(params: &Map<String, Value>) -> Result<u64, MethodError> {
    match params.get("since") {
        None | Some(Value::Null) => Ok(0),
        Some(v) => v
            .as_u64()
            .ok_or_else(|| MethodError::bad_params("since", "\"since\" is a whole number from 0")),
    }
}

/// End `run`: the controlled stop with `outcome` (`None`: the abort), raised on
/// the run's signals first so a fire in flight ends at its next block, then
/// queued.
pub(super) async fn stop_mimo_shock(
    run: &MsRun,
    outcome: Option<&'static str>,
    by: String,
    wait: bool,
) {
    if !run.view().state.is_live() {
        return;
    }
    run.signal(outcome, &by);
    let w = wait_for(run);
    match outcome {
        None => {
            let _ = run.jobs.ask(|reply| Cmd::Abort { by, reply }, w).await;
        }
        Some(outcome) if wait => {
            let _ = run
                .jobs
                .ask(
                    |reply| Cmd::Stop {
                        outcome,
                        by,
                        reply: Some(reply),
                    },
                    w,
                )
                .await;
        }
        Some(outcome) => {
            let _ = run.jobs.send(Cmd::Stop {
                outcome,
                by,
                reply: None,
            });
        }
    }
}
