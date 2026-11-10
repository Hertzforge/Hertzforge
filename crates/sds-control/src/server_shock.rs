//! The classical shock methods of the [`Service`] (schema/PROTOCOL.md, "Classical
//! Shock"): a configuration and its run beside the Random and MIMO slots, under the
//! same run exclusion and the same controller rules, on the transient-run rails.

use super::{Conn, Feed, Out, Service, DATA_SLACK, DISPLAY_PERIOD, LIVE_TICK};
use crate::codec::{self, Arrays};
use crate::laws;
use crate::protocol::{self, num, ErrorCode, MethodError};
use crate::pulse::rails::{Jobs, Shared};
use crate::shock::config::{self, CheckContext, LawFacts, WireShock};
use crate::shock::engine::{self, LevelMove};
use crate::shock::preview;
use crate::shock::run::{self as srun, ArmContext, Cmd, RunPlan};
use crate::shock::subscribe::{self, DataContext, Subscription};
use crate::shock::view::{self, LawRef, ShockView, State, StationRef};
use crate::shock::{RunData, ShockRun};
use crate::station::{self, Selection, StationPlan};
use crate::table;
use hertzforge_random_laws::{may_arm_on_hardware, LawCatalog, LawWorld};
use hertzforge_shock_common::ShockRunMode;
use hertzforge_shock_host::ClassicalLawFactory;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One connection's classical shock subscription and when it was last sent a frame.
pub struct ShockDisplay {
    sub: Arc<Subscription>,
    last: Option<Instant>,
}

/// How long to wait for the run thread to answer: its identification, a record,
/// and a margin.
fn wait_for(run: &ShockRun) -> Duration {
    let w = &run.wire;
    let record_s = run.reference.record_len as f64 / w.fs_hz.max(1.0);
    Duration::from_secs_f64(10.0 + w.sysid_duration_s.max(0.0) + 2.0 * record_s)
}

const ARM_WAIT: Duration = Duration::from_secs(120);

fn not_configured() -> MethodError {
    MethodError::named(
        ErrorCode::Refused,
        "not_configured",
        "Classical shock is not configured; configure a test first.",
    )
}

/// The law a configuration's checks look up by id: the catalog's entry.
fn law_facts(cat: &LawCatalog, id: &str) -> Option<LawFacts> {
    cat.entry(id).map(|e| laws::classical_facts(&e))
}

fn law_ref(f: &LawFacts) -> LawRef {
    LawRef {
        id: f.id.clone(),
        name: f.name.clone(),
        version: f.version.clone(),
        sha256: f.sha256.clone(),
    }
}

impl Service {
    pub(super) fn shock_displays(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<u64, ShockDisplay>> {
        match self.shock_displays.lock() {
            Ok(d) => d,
            Err(p) => p.into_inner(),
        }
    }

    fn station_ref(inner: &super::Inner) -> StationRef {
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

    /// Parse and check `config` against the present table: the configuration, its
    /// items, its derived values and its law.
    async fn shock_check(
        &self,
        config: &Value,
    ) -> Result<
        (
            WireShock,
            table::Report,
            Option<config::Derived>,
            Option<LawFacts>,
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
        let id = w.law.id.clone();
        let (w, report, derived, facts) = self
            .with_catalog(move |cat| {
                let lookup = |i: &str| law_facts(cat, i);
                let ctx = CheckContext {
                    table: &tbl,
                    law: &lookup,
                    timeout_s,
                };
                let (report, derived) = w.check(&ctx);
                let facts = law_facts(cat, &id);
                Ok((w, report, derived, facts))
            })
            .await?;
        let mut all = table_report;
        all.extend(report);
        let derived = if all.errors.is_empty() { derived } else { None };
        Ok((w, all, derived, facts))
    }

    pub(super) async fn shock_configure(
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
        let (w, report, derived, facts) = self.shock_check(config).await?;
        if dry_run {
            let mut arrays = Arrays::new();
            let views = match &derived {
                Some(_) => {
                    let r = preview::reference(&w.pulse(), w.standard, w.fs_hz);
                    preview::preview_json(&r, Some(subscribe::DEFAULT_POINTS), &mut arrays)["views"]
                        .clone()
                }
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
        let run = self.new_shock_run(&mut inner, w, &derived, &facts)?;
        let v = run.view();
        Ok((
            json!({
                "state": view::state_json(Some(&v)),
                "derived": derived.to_json(),
                "warnings": report.warnings.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
            }),
            Arrays::new(),
        ))
    }

    /// Create a run (replacing the one before) and start its thread.
    fn new_shock_run(
        &self,
        inner: &mut super::Inner,
        w: WireShock,
        derived: &config::Derived,
        facts: &LawFacts,
    ) -> Result<Arc<ShockRun>, MethodError> {
        inner.shock_runs += 1;
        let run_id = format!("s-{:04}", inner.shock_runs);
        let station = Self::station_ref(inner);
        let initial = view::configured_view(&run_id, &w, derived, station, law_ref(facts));
        let shared = Arc::new(Shared::new(initial.clone(), self.shock_notify.clone()));
        let data = Arc::new(RunData::default());
        let reference = Arc::new(preview::reference(&w.pulse(), w.standard, w.fs_hz));
        let raw = w.raw.clone();
        let wire = Arc::new(w);
        let plan = RunPlan {
            run_id: run_id.clone(),
            cfg: wire.clone(),
            engine: self
                .opts
                .shock_engine
                .clone()
                .unwrap_or_else(engine::service_engine_factory),
            sim_speed: self.opts.sim_speed,
            timeout_s: self.opts.timeout.as_secs_f64(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        srun::spawn(plan, initial.clone(), shared.clone(), data.clone(), rx)
            .map_err(|e| MethodError::internal(format!("could not start the run thread: {e}")))?;
        let run = Arc::new(ShockRun {
            id: run_id.clone(),
            shared: shared.clone(),
            data,
            config: raw,
            derived: derived.to_json(),
            reference,
            wire: wire.clone(),
            jobs: Jobs::new(&run_id, tx),
        });
        inner.shock = Some(run.clone());
        inner.shock_rows_sent = 0;
        shared.add_log(
            "state",
            format!("configured \"{}\" on {}", wire.title, initial.station.class),
        );
        let _ = self.shock_notify.send((run_id, Arc::new(initial)));
        Ok(run)
    }

    pub(super) async fn shock_config(&self) -> (Value, Arrays) {
        let inner = self.inner.lock().await;
        let mut arrays = Arrays::new();
        let result = match &inner.shock {
            None => json!({"run_id": null, "config": null, "derived": null, "views": []}),
            Some(run) => json!({
                "run_id": run.id,
                "config": run.config,
                "derived": run.derived,
                "views": preview::preview_json(
                    &run.reference,
                    Some(subscribe::DEFAULT_POINTS),
                    &mut arrays,
                )["views"],
            }),
        };
        (result, arrays)
    }

    pub(super) async fn shock_preview(
        &self,
        params: &Map<String, Value>,
    ) -> Result<(Value, Arrays), MethodError> {
        let pulse = config::parse_pulse(
            params
                .get("pulse")
                .ok_or_else(|| MethodError::bad_params("pulse", "A preview needs a pulse."))?,
            "pulse",
        )?;
        let (standard, _, _) = config::parse_standard(params.get("standard"), "standard")?;
        let fs_hz = protocol::named_f64(params, "fs_hz")?.unwrap_or(config::DEFAULT_FS_HZ);
        // The same checks a configuration's pulse gets.
        let probe = json!({
            "pulse": params["pulse"],
            "acquisition": {"fs_hz": fs_hz},
            "schedule": {"mode": "manual", "levels_db": [0], "pulses": [1]},
            "limits": {"drive_vpk_ceiling": 1, "drive_vpk_abort": 2},
        });
        let w = config::parse(&probe)?;
        let mut items = Vec::new();
        let tbl = {
            let mut t = table::Table::new().table;
            let mut c = table::new_row(1);
            c.role = hertzforge_channel_table::ChannelRole::Control;
            let mut d = table::new_row(2);
            d.role = hertzforge_channel_table::ChannelRole::Drive;
            t.channels = vec![c, d];
            t
        };
        let builtin = |id: &str| (id == "fd_correct").then(laws::builtin_classical_facts);
        let (r, _) = w.check(&CheckContext {
            table: &tbl,
            law: &builtin,
            timeout_s: f64::INFINITY,
        });
        for i in &r.errors {
            if let crate::table::At::Field(f) = &i.at {
                if f.starts_with("pulse") || f.starts_with("acquisition.fs_hz") {
                    items.push(i.clone());
                }
            }
        }
        if let Some(first) = items.first() {
            let field = match &first.at {
                crate::table::At::Field(f) => f.clone(),
                _ => "pulse".into(),
            };
            return Err(MethodError::bad_params(&field, first.message.clone()));
        }
        let r = tokio::task::spawn_blocking(move || preview::reference(&pulse, standard, fs_hz))
            .await
            .map_err(|e| MethodError::internal(format!("the preview failed: {e}")))?;
        let mut arrays = Arrays::new();
        let result = preview::preview_json(&r, Some(subscribe::DEFAULT_POINTS), &mut arrays);
        Ok((result, arrays))
    }

    async fn controlled_shock(&self, conn: &Conn) -> Result<Arc<ShockRun>, MethodError> {
        let inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.shock.clone().ok_or_else(not_configured)
    }

    /// What a run arms on now: the table, the selected station and its law.
    async fn shock_arm_context(&self, run: &ShockRun) -> Result<ArmContext, MethodError> {
        let (tbl, station_plan, hardware) = {
            let inner = self.inner.lock().await;
            let report = table::validate(&inner.table.table, &inner.ctx());
            if !report.errors.is_empty() {
                return Err(super::table_invalid(
                    &report,
                    "on this station, so the run cannot arm",
                ));
            }
            let fs_hz = run.wire.fs_hz;
            let (plan, hardware) = match inner.stations.selected() {
                Selection::Sim => (StationPlan::Sim { fs_hz }, false),
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
            (inner.table.table.clone(), plan, hardware)
        };
        let id = run.wire.law.id.clone();
        let wire = run.wire.clone();
        let values_of = id.clone();
        let law_values = self
            .with_catalog(move |cat| {
                Ok(cat
                    .entry(&values_of)
                    .map(|e| e.manifest.diagnostics)
                    .unwrap_or_default())
            })
            .await
            .unwrap_or_default();
        let law: Result<Arc<dyn ClassicalLawFactory>, MethodError> = self
            .with_catalog(move |cat| {
                if let Some(entry) = cat.entry(&id) {
                    if hardware && may_arm_on_hardware(&entry).is_err() {
                        return Err(MethodError::named(
                            ErrorCode::Refused,
                            "law_not_checked",
                            format!(
                                "The control law \"{}\" has no passed check for this file; run \
                                 laws.check on it before it drives hardware.",
                                entry.identity.name
                            ),
                        ));
                    }
                }
                let lf = law_facts(cat, &id).ok_or_else(|| {
                    MethodError::named(
                        ErrorCode::NotFound,
                        "law_unknown",
                        format!("No control law \"{id}\" is in the catalog."),
                    )
                })?;
                let choice = wire.law_choice(&lf);
                Ok(
                    hertzforge_shock_laws::resolve_classical(cat, &choice).map_err(|e| {
                        MethodError::named(
                            ErrorCode::Refused,
                            "arm_refused",
                            format!("The control law could not be built: {e}."),
                        )
                    }),
                )
            })
            .await?;
        Ok(ArmContext {
            table: tbl,
            station: station_plan,
            scanner: self.opts.scanner.clone(),
            data_root: self.opts.data_root.clone(),
            capture_root: self.opts.capture_root.clone(),
            bind: self.opts.bind.clone(),
            law,
            law_values,
        })
    }

    pub(super) async fn shock_arm(
        self: &Arc<Self>,
        conn: &Conn,
        params: &Map<String, Value>,
        start: bool,
    ) -> Result<Value, MethodError> {
        let ack =
            if start {
                None
            } else {
                match params.get("override_ack") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
                    Some(_) => return Err(MethodError::bad_params(
                        "override_ack",
                        "An override acknowledgement is the operator's own words; it cannot be \
                         empty.",
                    )),
                }
            };
        let run = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.shock.clone().ok_or_else(not_configured)?;
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
                || inner.srs_live()
                || inner.sine_live()
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
        let ctx = self.shock_arm_context(&run).await?;
        if start {
            run.jobs
                .ask(|reply| Cmd::Start { ctx, reply }, ARM_WAIT)
                .await?;
        } else {
            run.jobs
                .ask(|reply| Cmd::Arm { ack, ctx, reply }, ARM_WAIT)
                .await?;
        }
        self.spawn_shock_live(run.clone());
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn shock_proceed(&self, conn: &Conn) -> Result<Value, MethodError> {
        let run = self.controlled_shock(conn).await?;
        let v = run.view();
        if v.state != State::Review {
            return Err(srun::wrong_state(&v, "Proceeding"));
        }
        run.jobs
            .ask(|reply| Cmd::Proceed { reply }, wait_for(&run))
            .await?;
        Ok(view::state_json(Some(&run.view())))
    }

    /// The refusals a fire, train or burst gets without asking the run thread.
    fn pre_fire(run: &ShockRun, verb: &str) -> Result<(), MethodError> {
        if run.wire.mode == ShockRunMode::Auto {
            return Err(srun::wrong_mode(verb));
        }
        let v = run.view();
        if run.shared.progress.now().is_some() || v.phase == "firing" {
            return Err(srun::not_yet(
                run.reference.record_len as f64 / run.wire.fs_hz.max(1.0),
            ));
        }
        if !(v.state == State::Running && v.phase == "waiting") {
            return Err(srun::wrong_state(&v, verb));
        }
        Ok(())
    }

    pub(super) async fn shock_fire(&self, conn: &Conn) -> Result<Value, MethodError> {
        let run = self.controlled_shock(conn).await?;
        Self::pre_fire(&run, "A fire")?;
        run.jobs
            .ask(|reply| Cmd::Fire { reply }, wait_for(&run))
            .await?;
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn shock_train(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let run = self.controlled_shock(conn).await?;
        let count = match params.get("count") {
            None | Some(Value::Null) => run.wire.train_count,
            Some(v) => v
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| (1..=config::MAX_PULSES_PER_RUNG).contains(n))
                .ok_or_else(|| {
                    MethodError::bad_params(
                        "count",
                        format!(
                            "\"count\" is a whole number of pulses from 1 to {}",
                            config::MAX_PULSES_PER_RUNG
                        ),
                    )
                })?,
        };
        let spacing_s =
            protocol::named_f64(params, "spacing_s")?.unwrap_or(run.wire.train_spacing_s);
        let floor = run.wire.spacing_s();
        if count > 1 && !(spacing_s >= floor && spacing_s <= config::MAX_SPACING_S) {
            return Err(MethodError::bad_params(
                "spacing_s",
                format!(
                    "A train's spacing is at least the time between pulses ({} s) and at most {} s.",
                    num(config::round(floor, 4)),
                    num(config::MAX_SPACING_S)
                ),
            ));
        }
        Self::pre_fire(&run, "A train")?;
        run.jobs
            .ask(
                |reply| Cmd::Train {
                    count,
                    spacing_s,
                    reply,
                },
                wait_for(&run),
            )
            .await?;
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn shock_burst(&self, conn: &Conn) -> Result<Value, MethodError> {
        let run = self.controlled_shock(conn).await?;
        if run.wire.gunfire.is_none() {
            return Err(MethodError::named(
                ErrorCode::Refused,
                "wrong_state",
                "No gunfire burst is configured for this run.",
            ));
        }
        Self::pre_fire(&run, "A burst")?;
        run.jobs
            .ask(|reply| Cmd::Burst { reply }, wait_for(&run))
            .await?;
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn shock_level(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let delta = protocol::named_f64(params, "delta_db")?;
        let rung = match params.get("rung") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_u64().ok_or_else(|| {
                MethodError::bad_params("rung", "\"rung\" is a level of the table, from 0")
            })? as usize),
        };
        let to = match (delta, rung) {
            (Some(d), None) if d == 1.0 || d == -1.0 => LevelMove::Step(d),
            (None, Some(i)) => LevelMove::Rung(i),
            _ => {
                return Err(MethodError::bad_params(
                    "delta_db",
                    "A level change is a step of 1 or -1 dB (\"delta_db\"), or a level of the \
                     table (\"rung\").",
                ))
            }
        };
        let run = self.controlled_shock(conn).await?;
        if run.wire.mode == ShockRunMode::Auto {
            return Err(srun::wrong_mode("A level change"));
        }
        let level = run
            .jobs
            .ask(|reply| Cmd::Level { to, reply }, wait_for(&run))
            .await?;
        Ok(json!({"level_db": num(config::round(level, 2))}))
    }

    /// `shock.stop` and `shock.abort`: open to every connection.
    pub(super) async fn shock_end(&self, conn: &Conn, abort: bool) -> Result<Value, MethodError> {
        let (run, by) = {
            let inner = self.inner.lock().await;
            let Some(run) = inner.shock.clone() else {
                return Ok(view::state_json(None));
            };
            (run, inner.session.client_name(conn.id, &conn.peer))
        };
        if !run.view().state.is_live() {
            return Ok(view::state_json(Some(&run.view())));
        }
        stop_shock(
            &run,
            if abort { None } else { Some("operator_stop") },
            by,
            true,
        )
        .await;
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn shock_reset(&self, conn: &Conn) -> Result<Value, MethodError> {
        let (raw, busy) = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.shock.clone().ok_or_else(not_configured)?;
            (run.config.clone(), run.view().state.is_live())
        };
        if busy {
            self.inner.lock().await.run_active()?;
        }
        let (w, report, derived, facts) = self.shock_check(&raw).await?;
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.run_active()?;
        let (Some(derived), Some(facts)) = (derived, facts) else {
            return Err(config::config_invalid(&report));
        };
        let run = self.new_shock_run(&mut inner, w, &derived, &facts)?;
        let v = run.view();
        Ok(view::state_json(Some(&v)))
    }

    pub(super) async fn shock_clear(&self, conn: &Conn) -> Result<Value, MethodError> {
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        if inner.shock_live() {
            inner.run_active()?;
        }
        inner.shock = None;
        let state = view::state_json(None);
        inner.shock_state_key = view::state_key(None);
        self.emit("shock.state", state.clone());
        Ok(state)
    }

    pub(super) async fn shock_state_now(&self) -> Value {
        let inner = self.inner.lock().await;
        let v = inner.shock.as_ref().map(|r| r.view());
        view::state_json(v.as_deref())
    }

    pub(super) async fn shock_log(
        &self,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let since = since_param(params)?;
        let inner = self.inner.lock().await;
        let (entries, next) = match &inner.shock {
            Some(r) => r.log_since(since),
            None => (Vec::new(), 0),
        };
        Ok(json!({
            "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
            "next": next,
        }))
    }

    pub(super) async fn shock_pulses(
        &self,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let since = since_param(params)?;
        let run = self.inner.lock().await.shock.clone();
        let (rows, next) = match &run {
            Some(r) => r.data.rows_since(since),
            None => (Vec::new(), 0),
        };
        Ok(json!({"pulses": rows.iter().map(|r| r.to_json()).collect::<Vec<_>>(), "next": next}))
    }

    pub(super) async fn shock_pulse(
        &self,
        params: &Map<String, Value>,
    ) -> Result<(Value, Arrays), MethodError> {
        let index = params
            .get("index")
            .and_then(Value::as_u64)
            .ok_or_else(|| MethodError::bad_params("index", "\"index\" is a pulse, from 0"))?;
        let points = subscribe::parse_points(true, params.get("points"), "points")?;
        let run = self
            .inner
            .lock()
            .await
            .shock
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
                 row is in shock.pulses.",
                crate::pulse::ring::DEFAULT_CAPACITY
            ))
        })?;
        let mut arrays = Arrays::new();
        let mut views = vec![
            view::pulse_view(&fig.time, points, &mut arrays),
            view::drive_view(
                fig.time.start,
                fig.time.fs_hz,
                &fig.drive,
                points,
                &mut arrays,
            ),
        ];
        if let Some((r, m)) = &fig.velocity {
            views.push(view::kinematic_view(
                view::Kinematic::Velocity,
                fig.time.start,
                fig.time.fs_hz,
                r,
                Some(m),
                points,
                &mut arrays,
            ));
        }
        if let Some((r, m)) = &fig.displacement {
            views.push(view::kinematic_view(
                view::Kinematic::Displacement,
                fig.time.start,
                fig.time.fs_hz,
                r,
                Some(m),
                points,
                &mut arrays,
            ));
        }
        if let Some(s) = &fig.srs {
            views.push(view::srs_view(s, &mut arrays));
        }
        Ok((json!({"pulse": row.to_json(), "views": views}), arrays))
    }

    /// `shock.subscribe`: replace `conn`'s subscription, and send it the run's
    /// latest data at once when the run has any.
    pub(super) async fn shock_subscribe(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let sub = subscribe::parse(params)?.map(Arc::new);
        let result = subscribe::result_json(sub.as_deref());
        match &sub {
            None => {
                self.shock_displays().remove(&conn.id);
            }
            Some(sub) => {
                self.shock_displays().insert(
                    conn.id,
                    ShockDisplay {
                        sub: sub.clone(),
                        last: None,
                    },
                );
                let run = self.inner.lock().await.shock.clone();
                if let Some(run) = run.filter(|r| r.view().streaming) {
                    self.emit_shock_data(conn.id, &run, sub);
                    if let Some(d) = self.shock_displays().get_mut(&conn.id) {
                        d.last = Some(Instant::now());
                    }
                }
            }
        }
        Ok(result)
    }

    fn emit_shock_data(&self, conn: u64, run: &ShockRun, sub: &Subscription) {
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
        match codec::encode(&protocol::event("shock.data", seq, data), &arrays) {
            Ok(frame) => {
                let _ = self
                    .events
                    .send(Arc::new(Out::To(conn, Feed::Shock, frame)));
            }
            Err(e) => eprintln!("sds-control: shock.data frame not encoded: {e}"),
        }
    }

    /// Each subscribed connection whose frame is due (every one when `last`).
    fn shock_data_due(&self, run: &ShockRun, last: bool) {
        let now = Instant::now();
        let due: Vec<(u64, Arc<Subscription>)> = {
            let mut displays = self.shock_displays();
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
            self.emit_shock_data(conn, run, &sub);
        }
    }

    /// A run published `v`: `shock.state` when what it shows changed, its new log
    /// entries, a `shock.pulse` for each new pulse, and `shock.data` when there is
    /// something new to plot.
    pub(super) async fn shock_changed(&self, run_id: &str, v: &ShockView) {
        let mut inner = self.inner.lock().await;
        let Some(run) = inner.shock.clone() else {
            return;
        };
        if run.id != run_id {
            return;
        }
        let shown = run.view();
        let key = view::state_key(Some(&shown));
        if key != inner.shock_state_key {
            inner.shock_state_key = key;
            self.emit("shock.state", view::state_json(Some(&shown)));
        }
        let since = if inner.shock_log_sent.0 == run.id {
            inner.shock_log_sent.1
        } else {
            0
        };
        let (entries, next) = run.log_since(since);
        inner.shock_log_sent = (run.id.clone(), next);
        if !entries.is_empty() {
            self.emit(
                "shock.log",
                json!({
                    "run_id": run.id,
                    "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
                }),
            );
        }
        let rows_now = run.data.row_count();
        let first_new = inner.shock_rows_sent;
        inner.shock_rows_sent = rows_now;
        let ident_new = v.identification.is_some() && !inner.shock_ident_sent;
        inner.shock_ident_sent = v.identification.is_some();
        drop(inner);
        for index in first_new..rows_now {
            let (Some(row), fig) = (run.data.row(index), run.data.figures(index)) else {
                continue;
            };
            let mut arrays = Arrays::new();
            let views = fig.map_or_else(Vec::new, |f| {
                view::default_pulse_views(&f, Some(subscribe::DEFAULT_POINTS), &mut arrays)
            });
            self.emit_binary(
                "shock.pulse",
                json!({"run_id": run.id, "pulse": row.to_json(), "views": views}),
                &arrays,
            );
        }
        let finished = v.state == State::Finished;
        if rows_now > first_new || ident_new || finished {
            self.shock_data_due(&run, finished);
        }
    }

    /// Publish `run`'s `shock.live` five times a second until it has finished, then
    /// once more with the final state. One task per run.
    fn spawn_shock_live(self: &Arc<Self>, run: Arc<ShockRun>) {
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
                    .shock
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
                    svc.emit_binary("shock.live", view::live_json(&v), &Arrays::new());
                }
                if last {
                    break;
                }
            }
        });
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

/// End `run`: the controlled stop with `outcome` (`None`: the abort), raised on the
/// run's signals first so a fire in flight ends at its next block, then queued.
pub(super) async fn stop_shock(
    run: &ShockRun,
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

impl Service {
    /// The catalog id of the classical shock law `laws.check` names, by its `id` or
    /// the `path` of its file (imported first, for the controller only, as the
    /// check itself would), or `None` when it names no classical law: the Random
    /// check then answers, refusing what it cannot check by name.
    pub(super) async fn classical_law_id(
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
                (None, Some(p)) => laws::import(cat, &p)?["law"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                _ => return Ok(None),
            };
            Ok(cat
                .entry(&id)
                .filter(|e| e.world == LawWorld::ShockClassical)
                .map(|e| e.id))
        })
        .await
        .ok()
        .flatten()
    }

    /// `laws.check` of a classical shock law: every case of the classical check on its
    /// simulated station, one `laws.check` event as each starts and as it ends.
    pub(super) async fn shock_laws_check(
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
                let facts = law_facts(cat, &law_id)
                    .ok_or_else(|| crate::mimo::law_check::unknown_law(&law_id))?;
                for (k, v) in &given {
                    match facts.params.iter().find(|p| &p.key == k) {
                        None => return Err(bad_choice(&law_id, &format!("no parameter {k}"))),
                        Some(spec) => {
                            if let Some(why) = config::param_problem(spec, v) {
                                return Err(bad_choice(&law_id, &format!("{} {why}", spec.label)));
                            }
                        }
                    }
                }
                let choice = hertzforge_random_laws::LawChoice::new(
                    law_id.clone(),
                    crate::mimo::adapter::params_of(&facts.params, &given),
                );
                let factory = hertzforge_shock_laws::resolve_classical(cat, &choice)
                    .map_err(|e| bad_choice(&law_id, &e.to_string()))?;
                Ok((facts, factory))
            })
            .await?;
        let _slot = CheckSlot::take()?;
        let svc = self.clone();
        let check_id = facts.id.clone();
        let results = tokio::task::spawn_blocking(move || {
            crate::shock::law_check::run_check(&check_id, factory, &mut |event| {
                svc.emit("laws.check", event);
            })
        })
        .await
        .map_err(|e| MethodError::internal(format!("the law check failed: {e}")))??;
        let passed = !results.is_empty() && results.iter().all(|(_, v)| v.passed);
        let cases: Vec<Value> = results.iter().map(|(j, _)| j.clone()).collect();
        let (law, recorded) = if facts.builtin {
            let id = facts.id.clone();
            let law = self
                .with_catalog(move |cat| {
                    cat.entry(&id)
                        .map(|e| laws::entry_json(&e))
                        .ok_or_else(|| MethodError::internal("the built-in law left the catalog"))
                })
                .await?;
            (law, false)
        } else {
            let sha = facts.sha256.clone();
            let verdicts: Vec<_> = results.into_iter().map(|(_, v)| v).collect();
            let law = self
                .with_catalog(move |cat| {
                    let record =
                        crate::shock::law_check::check_record(sha, now_rfc3339(), verdicts);
                    cat.record_check(&record)
                        .map_err(|e| not_recorded(&e.to_string()))?;
                    let e = cat
                        .entry(&record.law_sha256)
                        .ok_or_else(|| MethodError::internal("the checked law left the catalog"))?;
                    Ok(laws::entry_json(&e))
                })
                .await?;
            (law, true)
        };
        Ok(json!({
            "law": law,
            "report": crate::shock::law_check::report_json(&cases, passed, recorded),
        }))
    }
}
