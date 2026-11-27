//! The MIMO Mixed Mode methods of the [`Service`] (schema/PROTOCOL.md, "MIMO Mixed
//! Mode"): a configuration and its run beside the other test types, under the same
//! run exclusion and the same controller rules. The methods are Mixed Mode's
//! (`mixed.*`) under `mimo_mixed.*`, with MIMO Random's reference verbs under the
//! same names; the verbs the controller cannot do are refused by name
//! ([`crate::mimo_mixed::NOT_BUILT`]).

use super::{Conn, Feed, Out, Service, DATA_SLACK, DISPLAY_PERIOD, LIVE_TICK};
use crate::codec::{self, Arrays};
use crate::laws;
use crate::mimo_mixed::config::{self, Checked, WireMimoMixed};
use crate::mimo_mixed::run::{self as mrun, ArmContext, Cmd, RunPlan, RunShared};
use crate::mimo_mixed::subscribe::{self, Subscription};
use crate::mimo_mixed::view::{self, State, StationRef};
use crate::mimo_mixed::{refuse, wrong_state, MimoMixedRun};
use crate::protocol::{self, num, ErrorCode, MethodError};
use crate::random::run::LevelTarget;
use crate::station::{self, Selection, StationPlan};
use crate::table;
use crate::views::{self, Axis, Role, Scale, Series};
use hertzforge_mimo_mixed::service::LawRole;
use hertzforge_mimo_mixed::MimoMixedConfig;
use hertzforge_random::RunMode;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One connection's MIMO Mixed Mode subscription and when it was last sent a frame.
pub struct MimoMixedDisplay {
    sub: Arc<Subscription>,
    last: Option<Instant>,
}

fn not_configured() -> MethodError {
    refuse(
        "not_configured",
        "MIMO Mixed Mode is not configured; configure a test first.",
    )
}

/// How long to wait for the run thread to answer: an identification, a margin.
const VERB_WAIT: Duration = Duration::from_secs(120);
const ARM_WAIT: Duration = Duration::from_secs(120);

impl Service {
    pub(super) fn mimo_mixed_displays(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<u64, MimoMixedDisplay>> {
        match self.mimo_mixed_displays.lock() {
            Ok(d) => d,
            Err(p) => p.into_inner(),
        }
    }

    fn mimo_mixed_station_ref(inner: &super::Inner) -> StationRef {
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

    /// Parse and check `config` against the present table and the catalog.
    async fn mimo_mixed_check(
        &self,
        config: &Value,
    ) -> Result<(WireMimoMixed, table::Report, Option<Checked>), MethodError> {
        let mut w = config::parse(config)?;
        let (tbl, table_report) = {
            let inner = self.inner.lock().await;
            w.structure = inner.stations.sim_models().sine_structure.clone();
            (
                inner.table.table.clone(),
                table::validate(&inner.table.table, &inner.ctx()),
            )
        };
        let (w, report, checked) = self
            .with_catalog(move |cat| {
                let (report, checked) = config::check(&w, &tbl, cat);
                Ok((w, report, checked))
            })
            .await?;
        let mut all = table_report;
        all.extend(report);
        let checked = if all.errors.is_empty() { checked } else { None };
        Ok((w, all, checked))
    }

    /// The configuration's view: each control's reference at full level with its
    /// alarm and abort lines.
    fn mimo_mixed_views(c: &Checked, arrays: &mut Arrays) -> Value {
        let d = &c.derived;
        let band = d.band;
        let x: Vec<f64> = (band.0..band.1).map(|l| l as f64 * d.df_hz).collect();
        let (reference, alarm, abort) = view::reference_lines(&c.core, band, 0.0);
        let mut series = Vec::new();
        for (i, ch) in c.core.controls.iter().enumerate() {
            let name = format!("control {ch}");
            series.push(Series::new(
                &name,
                Role::Reference,
                x.clone(),
                reference[i].clone(),
            ));
            series.push(Series::new(
                &format!("{name} alarm high"),
                Role::Tolerance,
                x.clone(),
                alarm[i].1.clone(),
            ));
            series.push(Series::new(
                &format!("{name} alarm low"),
                Role::Tolerance,
                x.clone(),
                alarm[i].0.clone(),
            ));
            series.push(Series::new(
                &format!("{name} abort high"),
                Role::Abort,
                x.clone(),
                abort[i].1.clone(),
            ));
            series.push(Series::new(
                &format!("{name} abort low"),
                Role::Abort,
                x.clone(),
                abort[i].0.clone(),
            ));
        }
        let unit = crate::random::record::psd_unit(&c.core.reference.unit);
        Value::Array(vec![views::spectrum(
            arrays,
            "Reference",
            &Axis::new("Frequency", Some("Hz"), Scale::Log),
            &Axis::new("PSD", Some(&unit), Scale::Log),
            series,
            Some("frequency"),
        )])
    }

    pub(super) async fn mimo_mixed_configure(
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
        let (w, report, checked) = self.mimo_mixed_check(config).await?;
        if dry_run {
            let mut arrays = Arrays::new();
            let views = match &checked {
                Some(c) => Self::mimo_mixed_views(c, &mut arrays),
                None => json!([]),
            };
            return Ok((
                json!({
                    "errors": report.errors.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
                    "warnings": report.warnings.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
                    "derived": checked.as_ref().map_or(Value::Null, |c| c.derived.to_json()),
                    "views": views,
                }),
                arrays,
            ));
        }
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.run_active()?;
        if inner.sim_running() {
            return Err(refuse(
                "run_active",
                "A simulation is running; stop it first.",
            ));
        }
        let Some(checked) = checked else {
            return Err(crate::shock::config::config_invalid(&report));
        };
        let run = self.new_mimo_mixed_run(&mut inner, w, checked)?;
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

    /// Create a run (replacing the one before).
    fn new_mimo_mixed_run(
        &self,
        inner: &mut super::Inner,
        w: WireMimoMixed,
        checked: Checked,
    ) -> Result<Arc<MimoMixedRun>, MethodError> {
        inner.mimo_mixed_runs += 1;
        let run_id = format!("mimo-mixed-{:04}", inner.mimo_mixed_runs);
        let station = Self::mimo_mixed_station_ref(inner);
        let unit = checked.core.reference.unit.clone();
        let initial = view::configured_view(
            &run_id,
            &w.title,
            &checked.core,
            &checked.derived,
            station,
            unit,
        );
        let raw = w.raw.clone();
        let title = w.title.clone();
        let derived = checked.derived.to_json();
        let shared = Arc::new(RunShared::new(
            initial.clone(),
            self.mimo_mixed_notify.clone(),
        ));
        let wire = Arc::new(w);
        let core = Arc::new(checked.core);
        let facts = Arc::new(checked.derived);
        let plan = RunPlan {
            run_id: run_id.clone(),
            wire: wire.clone(),
            core: core.clone(),
            facts: facts.clone(),
            sim_speed: self.opts.sim_speed,
            capture_root: Some(self.opts.records_root()),
            timeout_s: self.opts.timeout.as_secs_f64(),
            record: self.opts.record.clone(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        mrun::spawn(plan, initial.clone(), shared.clone(), rx)
            .map_err(|e| MethodError::internal(format!("could not start the run thread: {e}")))?;
        let run = Arc::new(MimoMixedRun::new(
            run_id.clone(),
            raw,
            derived,
            wire,
            core,
            facts,
            shared,
            tx,
        ));
        inner.mimo_mixed = Some(run.clone());
        run.add_log(
            "state",
            format!("configured \"{title}\" on {}", initial.station.class),
        );
        let _ = self.mimo_mixed_notify.send((run_id, Arc::new(initial)));
        Ok(run)
    }

    pub(super) async fn mimo_mixed_config(&self) -> (Value, Arrays) {
        let inner = self.inner.lock().await;
        let mut arrays = Arrays::new();
        let result = match &inner.mimo_mixed {
            None => json!({"run_id": null, "config": null, "derived": null, "views": []}),
            Some(run) => {
                let checked = Checked {
                    core: (*run.core).clone(),
                    derived: (*run.facts).clone(),
                };
                json!({
                    "run_id": run.id,
                    "config": run.config,
                    "derived": run.derived,
                    "views": Self::mimo_mixed_views(&checked, &mut arrays),
                })
            }
        };
        (result, arrays)
    }

    async fn controlled_mimo_mixed(&self, conn: &Conn) -> Result<Arc<MimoMixedRun>, MethodError> {
        let inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.mimo_mixed.clone().ok_or_else(not_configured)
    }

    /// `mimo_mixed.arm` and `mimo_mixed.start`: the run's gates, the laws resolved
    /// for the station's class (the hardware rule per role), then the controller
    /// arms on the selected station.
    pub(super) async fn mimo_mixed_arm(
        self: &Arc<Self>,
        conn: &Conn,
        start: bool,
    ) -> Result<Value, MethodError> {
        let (run, plan, hardware, tbl, revision, devices) = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.mimo_mixed.clone().ok_or_else(not_configured)?;
            let v = run.view();
            if v.state == State::Finished {
                return Err(refuse(
                    "run_finished",
                    "This run has finished; reset or configure again to run.",
                ));
            }
            // Another module's live run refuses; this run's own arm does not refuse
            // its start (the state check below decides that).
            if inner.any_live() && !(start && v.state == State::Armed) {
                inner.run_active()?;
            }
            if inner.sim_running() {
                return Err(refuse(
                    "run_active",
                    "A simulation is running; stop it first.",
                ));
            }
            if !(v.state == State::Configured || (v.state == State::Armed && start)) {
                return Err(wrong_state(&v, if start { "Starting" } else { "Arming" }));
            }
            inner.table.arm_gate()?;
            let report = table::validate(&inner.table.table, &inner.ctx());
            if !report.errors.is_empty() {
                return Err(super::table_invalid(
                    &report,
                    "on this station, so the run cannot arm",
                ));
            }
            let fs_hz = run.wire.fs_hz;
            let (plan, hardware) = match inner.stations.selected() {
                Selection::Sim => {
                    station::models_unchanged(
                        run.wire.structure.as_deref(),
                        inner.stations.sim_models().sine_structure.as_deref(),
                    )?;
                    (
                        sim_plan(run.wire.structure.as_deref(), &inner.table.table, fs_hz)?,
                        false,
                    )
                }
                Selection::Ni { devices } => {
                    let (inputs, outputs) =
                        hertzforge_station::physical_from_table(&inner.table.table);
                    (
                        StationPlan::Ni {
                            devices: devices.clone(),
                            inputs,
                            outputs,
                            fs_hz,
                        },
                        station::class_name(inner.stations.class()) == "ni_real",
                    )
                }
            };
            (
                run,
                plan,
                hardware,
                inner.table.table.clone(),
                inner.table.revision,
                inner.stations.devices().to_vec(),
            )
        };
        // The laws, looked up afresh: on hardware each role's law must clear its
        // role (`law_not_checked`); the simulator takes any law.
        let core = run.core.clone();
        let laws = self
            .with_catalog(move |cat| Ok(config::resolve_laws(cat, &core, hardware)))
            .await?;
        if let Err(e) = &laws {
            if e.reason == Some("law_not_checked") {
                return Err(e.clone());
            }
        }
        let ctx = ArmContext {
            table: tbl,
            station: plan,
            scanner: self.opts.scanner.clone(),
            data_root: self.opts.data_root.clone(),
            bind: self.opts.bind.clone(),
            laws,
            revision,
            devices,
        };
        if start {
            run.ask(|reply| Cmd::Start { ctx, reply }, ARM_WAIT).await?;
        } else {
            run.ask(|reply| Cmd::Arm { ctx, reply }, ARM_WAIT).await?;
        }
        self.spawn_mimo_mixed_live(run.clone());
        Ok(view::state_json(Some(&run.view())))
    }

    /// The verbs that act on a running test: refused by state until it runs.
    pub(super) async fn mimo_mixed_running_verb(
        &self,
        conn: &Conn,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        // The parameters are checked before the state, as Mixed Mode's are.
        match method {
            "mimo_mixed.level" => {
                let delta = protocol::named_f64(params, "delta_db")?;
                let to = protocol::named_f64(params, "to_db")?;
                if delta.is_some() == to.is_some() {
                    return Err(MethodError::bad_params(
                        "delta_db",
                        "A level change is a step (\"delta_db\") or a level (\"to_db\"), one of \
                         the two.",
                    ));
                }
            }
            "mimo_mixed.mode" => match protocol::named_str(params, "mode")? {
                Some("auto") | Some("manual") => {}
                _ => {
                    return Err(MethodError::bad_params(
                        "mode",
                        "\"mode\" is \"auto\" or \"manual\"",
                    ))
                }
            },
            "mimo_mixed.override" => {
                match params.get("ack") {
                    Some(Value::String(s)) if !s.trim().is_empty() => {}
                    _ => {
                        return Err(MethodError::bad_params(
                            "ack",
                            "An override is the operator's own words on why the run should go \
                             on; it cannot be empty.",
                        ))
                    }
                }
                match params.get("reason") {
                    None | Some(Value::Null) => {}
                    Some(Value::String(r)) if r == "gate_failed" || r == "prediction_exceeded" => {}
                    Some(_) => {
                        return Err(MethodError::bad_params(
                            "reason",
                            "\"reason\" is \"gate_failed\" or \"prediction_exceeded\": what the \
                             override acknowledges",
                        ))
                    }
                }
            }
            "mimo_mixed.part" => {
                protocol::named_str(params, "id")?
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        MethodError::bad_params("id", "Switching a part names it by \"id\".")
                    })?;
                if !matches!(params.get("on"), Some(Value::Bool(_))) {
                    return Err(MethodError::bad_params(
                        "on",
                        "\"on\" is true (switch the part on) or false (switch it off).",
                    ));
                }
            }
            _ => {}
        }
        let run = self.controlled_mimo_mixed(conn).await?;
        let v = run.view();
        let state = || view::state_json(Some(&run.view()));
        match method {
            "mimo_mixed.proceed" => {
                if v.state != State::Review {
                    return Err(wrong_state(&v, "Proceeding"));
                }
                run.ask(|reply| Cmd::Proceed { reply }, VERB_WAIT).await?;
                Ok(state())
            }
            "mimo_mixed.level" => {
                let target = match (
                    protocol::named_f64(params, "delta_db")?,
                    protocol::named_f64(params, "to_db")?,
                ) {
                    (Some(d), _) => LevelTarget::Delta(d),
                    (_, Some(t)) => LevelTarget::To(t),
                    _ => unreachable!("checked above"),
                };
                let level = run
                    .ask(|reply| Cmd::Level { target, reply }, VERB_WAIT)
                    .await?;
                Ok(json!({"level_db": num(config::round(level, 2))}))
            }
            "mimo_mixed.mode" => {
                let mode = match protocol::named_str(params, "mode")? {
                    Some("manual") => RunMode::Manual,
                    _ => RunMode::Auto,
                };
                run.ask(|reply| Cmd::Mode { mode, reply }, VERB_WAIT)
                    .await?;
                Ok(json!({"mode": crate::random::view::mode_name(mode)}))
            }
            "mimo_mixed.override" => {
                let ack = params
                    .get("ack")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                let reason = match params.get("reason").and_then(Value::as_str) {
                    Some("gate_failed") => Some("gate_failed"),
                    Some("prediction_exceeded") => Some("prediction_exceeded"),
                    _ => None,
                };
                let by = {
                    let inner = self.inner.lock().await;
                    inner.session.client_name(conn.id, &conn.peer)
                };
                run.ask(
                    |reply| Cmd::Override {
                        ack,
                        by,
                        reason,
                        reply,
                    },
                    VERB_WAIT,
                )
                .await?;
                Ok(state())
            }
            "mimo_mixed.hold" | "mimo_mixed.resume" => {
                let on = method == "mimo_mixed.hold";
                run.ask(|reply| Cmd::Hold { on, reply }, VERB_WAIT).await?;
                Ok(state())
            }
            _ => {
                let id = params
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let on = params.get("on") == Some(&Value::Bool(true));
                let by = {
                    let inner = self.inner.lock().await;
                    inner.session.client_name(conn.id, &conn.peer)
                };
                let i = id.clone();
                let got = run
                    .ask(
                        |reply| Cmd::Part {
                            id: i,
                            on,
                            by,
                            reply,
                        },
                        VERB_WAIT,
                    )
                    .await?;
                Ok(json!({"id": id, "on": got}))
            }
        }
    }

    /// `mimo_mixed.stop` and `mimo_mixed.abort`: open to every connection.
    pub(super) async fn mimo_mixed_end(&self, conn: &Conn, abort: bool) -> Value {
        let (run, by) = {
            let inner = self.inner.lock().await;
            let Some(run) = inner.mimo_mixed.clone() else {
                return view::state_json(None);
            };
            (run, inner.session.client_name(conn.id, &conn.peer))
        };
        if run.view().state.is_live() {
            stop_mimo_mixed(
                &run,
                if abort { None } else { Some("operator_stop") },
                by,
                true,
            )
            .await;
        }
        view::state_json(Some(&run.view()))
    }

    pub(super) async fn mimo_mixed_reset(&self, conn: &Conn) -> Result<Value, MethodError> {
        let raw = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.mimo_mixed.clone().ok_or_else(not_configured)?;
            if run.view().state.is_live() {
                inner.run_active()?;
            }
            run.config.clone()
        };
        let (w, report, checked) = self.mimo_mixed_check(&raw).await?;
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.run_active()?;
        let Some(checked) = checked else {
            return Err(crate::shock::config::config_invalid(&report));
        };
        let run = self.new_mimo_mixed_run(&mut inner, w, checked)?;
        let v = run.view();
        Ok(view::state_json(Some(&v)))
    }

    pub(super) async fn mimo_mixed_clear(&self, conn: &Conn) -> Result<Value, MethodError> {
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        if inner.mimo_mixed_live() {
            inner.run_active()?;
        }
        inner.mimo_mixed = None;
        let state = view::state_json(None);
        inner.mimo_mixed_state_key = view::state_key(None);
        self.emit("mimo_mixed.state", state.clone());
        Ok(state)
    }

    pub(super) async fn mimo_mixed_state_now(&self) -> Value {
        let inner = self.inner.lock().await;
        let v = inner.mimo_mixed.as_ref().map(|r| r.view());
        view::state_json(v.as_deref())
    }

    pub(super) async fn mimo_mixed_log(
        &self,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let since = match params.get("since") {
            None | Some(Value::Null) => 0,
            Some(v) => v.as_u64().ok_or_else(|| {
                MethodError::bad_params("since", "\"since\" is a whole number from 0")
            })?,
        };
        let inner = self.inner.lock().await;
        let (entries, next) = match &inner.mimo_mixed {
            Some(r) => r.log_since(since),
            None => (Vec::new(), 0),
        };
        Ok(json!({
            "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
            "next": next,
        }))
    }

    /// MIMO Random's reference verbs under `mimo_mixed.reference.*`: the same
    /// handlers, the same results.
    pub(super) async fn mimo_mixed_reference(
        &self,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<(Value, Arrays), MethodError> {
        let verb = method.trim_start_matches("mimo_mixed.reference.");
        if verb == "build_rigid_body" {
            return crate::mimo::reference::build_rigid_body(params).map(|v| (v, Arrays::new()));
        }
        self.mimo_reference(&format!("mimo_random.reference.{verb}"), params)
            .await
    }

    /// `mimo_mixed.subscribe`: replace `conn`'s subscription, and send it the run's
    /// latest data at once when the run is streaming.
    pub(super) async fn mimo_mixed_subscribe(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let sub = subscribe::parse(params)?.map(Arc::new);
        let result = subscribe::result_json(sub.as_deref());
        match &sub {
            None => {
                self.mimo_mixed_displays().remove(&conn.id);
            }
            Some(sub) => {
                self.mimo_mixed_displays().insert(
                    conn.id,
                    MimoMixedDisplay {
                        sub: sub.clone(),
                        last: None,
                    },
                );
                let run = self.inner.lock().await.mimo_mixed.clone();
                if let Some(run) = run.filter(|r| r.view().streaming) {
                    self.emit_mimo_mixed_data(conn.id, &run, sub);
                    if let Some(d) = self.mimo_mixed_displays().get_mut(&conn.id) {
                        d.last = Some(Instant::now());
                    }
                }
            }
        }
        Ok(result)
    }

    fn emit_mimo_mixed_data(&self, conn: u64, run: &MimoMixedRun, sub: &Subscription) {
        let v = run.view();
        let mut arrays = Arrays::new();
        let data = subscribe::data_json(&v, sub, &mut arrays);
        let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match codec::encode(&protocol::event("mimo_mixed.data", seq, data), &arrays) {
            Ok(frame) => {
                let _ = self
                    .events
                    .send(Arc::new(Out::To(conn, Feed::MimoMixed, frame)));
            }
            Err(e) => eprintln!("sds-control: mimo_mixed.data frame not encoded: {e}"),
        }
    }

    fn mimo_mixed_data_due(&self, run: &MimoMixedRun, last: bool) {
        let now = Instant::now();
        let due: Vec<(u64, Arc<Subscription>)> = {
            let mut displays = self.mimo_mixed_displays();
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
            self.emit_mimo_mixed_data(conn, run, &sub);
        }
    }

    /// A run published: `mimo_mixed.state` when what it shows changed, and its new
    /// log entries. A publish of a run that is no longer the present one is dropped.
    pub(super) async fn mimo_mixed_changed(&self, run_id: &str) {
        let mut inner = self.inner.lock().await;
        let Some(run) = inner.mimo_mixed.clone() else {
            return;
        };
        if run.id != run_id {
            return;
        }
        let shown = run.view();
        let key = view::state_key(Some(&shown));
        if key != inner.mimo_mixed_state_key {
            inner.mimo_mixed_state_key = key;
            self.emit("mimo_mixed.state", view::state_json(Some(&shown)));
        }
        let since = if inner.mimo_mixed_log_sent.0 == run.id {
            inner.mimo_mixed_log_sent.1
        } else {
            0
        };
        let (entries, next) = run.log_since(since);
        inner.mimo_mixed_log_sent = (run.id.clone(), next);
        if !entries.is_empty() {
            self.emit(
                "mimo_mixed.log",
                json!({
                    "run_id": run.id,
                    "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
                }),
            );
        }
    }

    /// Publish `run`'s `mimo_mixed.live` five times a second while it streams, and
    /// each subscription's `mimo_mixed.data` at its own rate, until it has
    /// finished; then once more with the final state. One task per run.
    pub(super) fn spawn_mimo_mixed_live(self: &Arc<Self>, run: Arc<MimoMixedRun>) {
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
                    .mimo_mixed
                    .as_ref()
                    .is_some_and(|r| Arc::ptr_eq(r, &run));
                let v = run.view();
                if !v.streaming {
                    if !current || v.state == State::Finished {
                        break;
                    }
                    continue;
                }
                let last = v.state == State::Finished || !current;
                let now = Instant::now();
                if last || last_live.is_none_or(|t| now >= t + DISPLAY_PERIOD - DATA_SLACK) {
                    last_live = Some(now);
                    let mut arrays = Arrays::new();
                    let data = view::live_json(&v, &mut arrays);
                    let seq = svc.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    match codec::encode(&protocol::event("mimo_mixed.live", seq, data), &arrays) {
                        Ok(frame) => {
                            let _ = svc
                                .events
                                .send(Arc::new(Out::DefaultLive(Feed::MimoMixed, frame)));
                        }
                        Err(e) => {
                            eprintln!("sds-control: mimo_mixed.live frame not encoded: {e}")
                        }
                    }
                }
                svc.mimo_mixed_data_due(&run, last);
                if last {
                    break;
                }
            }
        });
    }

    /// `laws.check` with `mimo_mixed`: the MIMO Mixed check set with the law in its
    /// role and the built-in in the other. A law file's result is recorded for its
    /// role; a built-in's is reported only.
    pub(super) async fn mimo_mixed_laws_check(
        self: &Arc<Self>,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        use crate::mimo::law_check::{bad_choice, CheckSlot};
        let role = match params.get("mimo_mixed").and_then(Value::as_str) {
            Some("floor") => LawRole::Floor,
            Some("tones") => LawRole::Tones,
            _ => {
                return Err(MethodError::bad_params(
                    "mimo_mixed",
                    "\"mimo_mixed\" is the law's role, \"floor\" or \"tones\".",
                ))
            }
        };
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let id = protocol::named_str(params, "id")?.map(str::to_owned);
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
        let (facts, laws) = self
            .with_catalog(move |cat| {
                let id =
                    match (id, path) {
                        (Some(id), None) => id,
                        (None, Some(p)) => laws::import(cat, &p)?["law"]["id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                        _ => return Err(MethodError::bad_params(
                            "id",
                            "Checking a law needs its catalog \"id\" or the \"path\" of its file.",
                        )),
                    };
                let facts = match role {
                    LawRole::Floor => config::floor_law(cat, &id),
                    LawRole::Tones => config::tone_law(cat, &id),
                }
                .map_err(|(why, words)| match why {
                    "law_unknown" => crate::mimo::law_check::unknown_law(&id),
                    // A law of another world is no law for this role.
                    _ => MethodError::named(ErrorCode::BadRequest, why, words)
                        .with_detail(json!({"id": id, "role": config::role_name(role)})),
                })?;
                // The check's own configuration, with the law under check in its
                // role and the built-in default in the other.
                let mut cfg = hertzforge_mimo_mixed_laws::mimo_mixed_check_config(
                    hertzforge_mimo_mixed_laws::MimoMixedCheckCase::ALL[0],
                );
                let pick = match role {
                    LawRole::Floor => &mut cfg.law.floor,
                    LawRole::Tones => &mut cfg.law.tones,
                };
                pick.id = id.clone();
                pick.params = given;
                let laws = config::resolve_laws(cat, &cfg, false)
                    .map_err(|e| bad_choice(&id, &e.message))?;
                Ok((facts, laws))
            })
            .await?;
        let _slot = CheckSlot::take()?;
        let svc = self.clone();
        let law_id = facts.id.clone();
        let clocks = self.opts.mimo_mixed_check_clocks.clone();
        let results = tokio::task::spawn_blocking(move || {
            crate::mimo_mixed::law_check::run_check(
                &law_id,
                role,
                &laws,
                clocks.as_ref(),
                &mut |event| {
                    svc.emit("laws.check", event);
                },
            )
        })
        .await
        .map_err(|e| MethodError::internal(format!("the law check failed: {e}")))??;
        // The verdict is the required cases'; an informational case is run
        // and recorded beside it.
        let passed = !results.is_empty()
            && results.iter().all(|(_, v)| {
                v.passed
                    || hertzforge_mimo_mixed_laws::MimoMixedCheckCase::from_name(&v.name)
                        .is_some_and(|c| !c.required())
            });
        let cases: Vec<Value> = results.iter().map(|(j, _)| j.clone()).collect();
        let (facts, recorded) = if facts.builtin {
            (facts, false)
        } else {
            let id = facts.id.clone();
            let sha = facts.sha256.clone();
            let verdicts: Vec<_> = results.into_iter().map(|(_, v)| v).collect();
            let facts = self
                .with_catalog(move |cat| {
                    let record = hertzforge_mimo_mixed_laws::mimo_mixed_check_record(
                        &sha,
                        &crate::mimo::law_check::now_rfc3339(),
                        verdicts,
                    );
                    hertzforge_mimo_mixed_laws::record_mimo_mixed_check(cat, role, &record)
                        .map_err(|e| crate::mimo::law_check::not_recorded(&e))?;
                    match role {
                        LawRole::Floor => config::floor_law(cat, &id),
                        LawRole::Tones => config::tone_law(cat, &id),
                    }
                    .map_err(|_| MethodError::internal("the checked law left the catalog"))
                })
                .await?;
            (facts, true)
        };
        Ok(json!({
            "law": facts.to_json(),
            "report": {
                "passed": passed,
                "recorded": recorded,
                "role": config::role_name(role),
                "tool_version": hertzforge_mimo_mixed_laws::MIMO_MIXED_CHECK_VERSION,
                "cases": cases,
            },
        }))
    }
}

/// The simulator's station: the named sine structure, or the standard one of the
/// table's size; its inputs and drives must be the table's.
fn sim_plan(
    structure: Option<&str>,
    table: &hertzforge_channel_table::ChannelTable,
    fs_hz: f64,
) -> Result<StationPlan, MethodError> {
    use hertzforge_channel_table::ChannelRole;
    let inputs = table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role != ChannelRole::Drive)
        .count();
    let drives = table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role == ChannelRole::Drive)
        .count();
    let refused = |words: String| MethodError::named(ErrorCode::Refused, "arm_refused", words);
    let s = match structure {
        Some(name) => hertzforge_station::SimSineStructure::from_name(name)
            .ok_or_else(|| refused(format!("The simulator has no structure \"{name}\".")))?,
        None => crate::mimo_sine::config::default_structure(inputs, drives).ok_or_else(|| {
            refused(format!(
                "The simulator has no sine structure of {inputs} inputs and {drives} drives; \
                 choose one in the station's simulator models or size the table to one."
            ))
        })?,
    };
    let (n_in, n_d) = s.dims();
    if (inputs, drives) != (n_in, n_d) {
        return Err(refused(format!(
            "The simulator's structure \"{}\" has {n_in} inputs and {n_d} drives; the table \
             has {inputs} enabled inputs and {drives} drives.",
            s.name()
        ))
        .with_detail(json!({"structure": s.name(), "inputs": n_in, "drives": n_d})));
    }
    Ok(StationPlan::Sim {
        fs_hz,
        plant: Some(hertzforge_station::SimPlant::SineStructure(s)),
        outputs: Some(n_d as u32),
    })
}

/// End `run`: the controlled stop with `outcome` (`None`: the abort), raised on the
/// run's signals first, then queued.
pub(super) async fn stop_mimo_mixed(
    run: &MimoMixedRun,
    outcome: Option<&'static str>,
    by: String,
    wait: bool,
) {
    if !run.view().state.is_live() {
        return;
    }
    run.shared.signal_end(outcome.is_none(), &by);
    match outcome {
        None => {
            let _ = run.ask(|reply| Cmd::Abort { by, reply }, VERB_WAIT).await;
        }
        Some(outcome) if wait => {
            let _ = run
                .ask(
                    |reply| Cmd::Stop {
                        outcome,
                        by,
                        reply: Some(reply),
                    },
                    VERB_WAIT,
                )
                .await;
        }
        Some(outcome) => {
            let _ = run.send(Cmd::Stop {
                outcome,
                by,
                reply: None,
            });
        }
    }
}

/// How long the shutdown waits for a MIMO Mixed Mode run's controlled stop before
/// it aborts: at least ten seconds and twice its stop ramp.
pub(super) fn shutdown_patience(run: &MimoMixedRun) -> Duration {
    let c: &MimoMixedConfig = &run.core;
    let ramp = c.shutdown_ramp_s.max(c.part_ramp_s);
    Duration::from_secs_f64(10f64.max(2.0 * ramp + 2.0))
}
