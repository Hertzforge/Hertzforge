//! The Sine methods of the [`Service`] (schema/PROTOCOL.md, "Sine"): a
//! configuration and its run beside the Random, MIMO, classical and SRS shock
//! slots, under the same run exclusion and the same controller rules.

use super::{Conn, Feed, Out, Service, DATA_SLACK, DISPLAY_PERIOD, LIVE_TICK};
use crate::codec::{self, Arrays};
use crate::laws;
use crate::protocol::{self, num, ErrorCode, MethodError};
use crate::random::run::LevelTarget;
use crate::sine::config::{self, CheckContext, LawFacts, WireSine};
use crate::sine::run::{self as srun, ArmContext, Cmd, RunPlan, RunShared};
use crate::sine::subscribe::{self, Subscription};
use crate::sine::view::{self, State, StationRef};
use crate::sine::{profile, SineRun};
use crate::station::{self, Selection, StationPlan};
use crate::table;
use hertzforge_random_laws::{may_arm_on_hardware, LawWorld};
use hertzforge_sine::{RunMode, SineLawFactory};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One connection's Sine subscription and when it was last sent a frame.
pub struct SineDisplay {
    sub: Arc<Subscription>,
    last: Option<Instant>,
}

/// How long to wait for the run thread to answer: its identification, a margin.
fn wait_for(run: &SineRun) -> Duration {
    let fs = run.wire.fs_hz().max(1.0);
    Duration::from_secs_f64(10.0 + 2.0 * run.wire.identification_s(fs))
}

const ARM_WAIT: Duration = Duration::from_secs(120);

fn not_configured() -> MethodError {
    MethodError::named(
        ErrorCode::Refused,
        "not_configured",
        "Sine is not configured; configure a test first.",
    )
}

impl Service {
    pub(super) fn sine_displays(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<u64, SineDisplay>> {
        match self.sine_displays.lock() {
            Ok(d) => d,
            Err(p) => p.into_inner(),
        }
    }

    fn sine_station_ref(inner: &super::Inner) -> StationRef {
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

    /// Parse and check `config` against the present table and station: the
    /// configuration, its items, its derived values and its law.
    async fn sine_check(
        &self,
        config: &Value,
    ) -> Result<
        (
            WireSine,
            table::Report,
            Option<config::Derived>,
            Option<LawFacts>,
        ),
        MethodError,
    > {
        let w = config::parse(config)?;
        let (tbl, table_report, outputs) = {
            let inner = self.inner.lock().await;
            let outputs = match inner.stations.selected() {
                Selection::Sim => station::SIM_OUTPUTS,
                Selection::Ni { .. } => WireSine::drive_channels(&inner.table.table).len() as u32,
            };
            (
                inner.table.table.clone(),
                table::validate(&inner.table.table, &inner.ctx()),
                outputs,
            )
        };
        let timeout_s = self.opts.timeout.as_secs_f64();
        let id = w.law.id.clone();
        let (w, report, derived, facts) = self
            .with_catalog(move |cat| {
                let lookup = |i: &str| laws::sine_lookup(Some(cat), i);
                let ctx = CheckContext {
                    table: &tbl,
                    law: &lookup,
                    timeout_s,
                    station_outputs: outputs,
                };
                let (report, derived) = w.check(&ctx);
                let facts = laws::sine_lookup(Some(cat), &id);
                Ok((w, report, derived, facts))
            })
            .await?;
        let mut all = table_report;
        all.extend(report);
        let derived = if all.errors.is_empty() { derived } else { None };
        Ok((w, all, derived, facts))
    }

    /// The configuration's three profile views over its band.
    fn sine_views(w: &WireSine, arrays: &mut Arrays) -> Value {
        match profile::figures(&w.profile, w.test.band()) {
            Some(f) => Value::Array(profile::profile_views(&f, arrays)),
            None => json!([]),
        }
    }

    pub(super) async fn sine_configure(
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
        let (w, report, derived, facts) = self.sine_check(config).await?;
        if dry_run {
            let mut arrays = Arrays::new();
            let views = match &derived {
                Some(_) => Self::sine_views(&w, &mut arrays),
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
        let run = self.new_sine_run(&mut inner, w, &derived, facts)?;
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
    fn new_sine_run(
        &self,
        inner: &mut super::Inner,
        w: WireSine,
        derived: &config::Derived,
        facts: LawFacts,
    ) -> Result<Arc<SineRun>, MethodError> {
        inner.sine_runs += 1;
        let run_id = format!("sine-{:04}", inner.sine_runs);
        let station = Self::sine_station_ref(inner);
        let live: Vec<(String, f64)> = facts
            .live
            .iter()
            .map(|k| {
                let v = w
                    .law
                    .params
                    .get(k)
                    .and_then(Value::as_f64)
                    .or_else(|| {
                        facts
                            .params
                            .iter()
                            .find(|p| &p.key == k)
                            .and_then(|p| match &p.kind {
                                hertzforge_law_host::ParamKind::Number(s) => Some(s.default),
                                _ => None,
                            })
                    })
                    .unwrap_or(0.0);
                (k.clone(), v)
            })
            .collect();
        let labels = self.law_labels(&facts);
        let law = view::law_state(config::LawRef::of(&facts), live, labels);
        let initial = view::configured_view(&run_id, &w, derived.grid, station, Some(law));
        let shared = Arc::new(RunShared::new(initial.clone(), self.sine_notify.clone()));
        let raw = w.raw.clone();
        let wire = Arc::new(w);
        let plan = RunPlan {
            run_id: run_id.clone(),
            config: wire.clone(),
            law: facts.clone(),
            sim_speed: self.opts.sim_speed,
            capture_root: self.opts.capture_root.clone(),
            timeout_s: self.opts.timeout.as_secs_f64(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        srun::spawn(plan, initial.clone(), shared.clone(), rx)
            .map_err(|e| MethodError::internal(format!("could not start the run thread: {e}")))?;
        let run = Arc::new(SineRun::new(
            run_id.clone(),
            shared.clone(),
            tx,
            raw,
            derived.to_json(),
            wire.clone(),
            facts,
        ));
        inner.sine = Some(run.clone());
        shared.add_log(
            "state",
            format!("configured \"{}\" on {}", wire.title, initial.station.class),
        );
        let _ = self.sine_notify.send((run_id, Arc::new(initial)));
        Ok(run)
    }

    /// The declared values' labels and units, by key, for the Law reading.
    fn law_labels(&self, facts: &LawFacts) -> Vec<(String, String, String)> {
        let m = if facts.builtin {
            Some(laws::sine_builtin_manifest())
        } else {
            self.catalog
                .lock()
                .ok()
                .and_then(|c| c.as_ref().and_then(|c| c.entry(&facts.id)))
                .map(|e| e.manifest)
        };
        m.map(|m| {
            m.diagnostics
                .iter()
                .map(|d| (d.key.clone(), d.label.clone(), d.unit.clone()))
                .collect()
        })
        .unwrap_or_default()
    }

    pub(super) async fn sine_config(&self) -> (Value, Arrays) {
        let inner = self.inner.lock().await;
        let mut arrays = Arrays::new();
        let result = match &inner.sine {
            None => json!({"run_id": null, "config": null, "derived": null, "views": []}),
            Some(run) => json!({
                "run_id": run.id,
                "config": run.config,
                "derived": run.derived,
                "views": Self::sine_views(&run.wire, &mut arrays),
            }),
        };
        (result, arrays)
    }

    pub(super) async fn sine_profile_import(
        &self,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let params = params.clone();
        tokio::task::spawn_blocking(move || profile::import(&params))
            .await
            .map_err(|e| MethodError::internal(format!("the import failed: {e}")))?
    }

    pub(super) async fn sine_profile_validate(
        &self,
        params: &Map<String, Value>,
    ) -> Result<(Value, Arrays), MethodError> {
        let mut arrays = Arrays::new();
        let result = profile::validate(params, &mut arrays)?;
        Ok((result, arrays))
    }

    async fn controlled_sine(&self, conn: &Conn) -> Result<Arc<SineRun>, MethodError> {
        let inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.sine.clone().ok_or_else(not_configured)
    }

    /// What a run arms on now: the table, the selected station and its law.
    async fn sine_arm_context(&self, run: &SineRun) -> Result<ArmContext, MethodError> {
        let (tbl, station_plan, hardware) = {
            let inner = self.inner.lock().await;
            let report = table::validate(&inner.table.table, &inner.ctx());
            if !report.errors.is_empty() {
                return Err(super::table_invalid(
                    &report,
                    "on this station, so the run cannot arm",
                ));
            }
            let fs_hz = run.wire.fs_hz();
            let (plan, hardware) = match inner.stations.selected() {
                Selection::Sim => (StationPlan::sim(fs_hz), false),
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
        let facts = run.law.clone();
        let law: Result<Arc<dyn SineLawFactory>, MethodError> = self
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
                let choice = wire.law_choice(&facts);
                Ok(
                    hertzforge_sine_laws::resolve_sine(cat, &choice).map_err(|e| {
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
            bind: self.opts.bind.clone(),
            law,
        })
    }

    pub(super) async fn sine_arm(
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
            let run = inner.sine.clone().ok_or_else(not_configured)?;
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
                || inner.mixed_live()
                || inner.twr_live()
                || inner.explorer_live()
                || inner.recorder_live()
                || inner.mimo_shock_live()
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
        let ctx = self.sine_arm_context(&run).await?;
        if start {
            run.ask(|reply| Cmd::Start { ctx, reply }, ARM_WAIT).await?;
        } else {
            run.ask(|reply| Cmd::Arm { ack, ctx, reply }, ARM_WAIT)
                .await?;
        }
        self.spawn_sine_live(run.clone());
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn sine_proceed(&self, conn: &Conn) -> Result<Value, MethodError> {
        let run = self.controlled_sine(conn).await?;
        let v = run.view();
        if v.state != State::Review {
            return Err(srun::wrong_state(&v, "Proceeding"));
        }
        run.ask(|reply| Cmd::Proceed { reply }, wait_for(&run))
            .await?;
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn sine_level(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let delta = protocol::named_f64(params, "delta_db")?;
        let to = protocol::named_f64(params, "to_db")?;
        let target = match (delta, to) {
            (Some(d), None) => LevelTarget::Delta(d),
            (None, Some(t)) => LevelTarget::To(t),
            _ => {
                return Err(MethodError::bad_params(
                    "delta_db",
                    "A level change is a step (\"delta_db\") or a level of the schedule \
                     (\"to_db\"), one of the two.",
                ))
            }
        };
        let run = self.controlled_sine(conn).await?;
        let level = run
            .ask(|reply| Cmd::Level { target, reply }, wait_for(&run))
            .await?;
        Ok(json!({"level_db": num(config::round(level, 2))}))
    }

    pub(super) async fn sine_mode(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let mode = match protocol::named_str(params, "mode")? {
            Some("auto") => RunMode::Auto,
            Some("manual") => RunMode::Manual,
            _ => {
                return Err(MethodError::bad_params(
                    "mode",
                    "\"mode\" is \"auto\" or \"manual\"",
                ))
            }
        };
        let run = self.controlled_sine(conn).await?;
        run.ask(|reply| Cmd::Mode { mode, reply }, wait_for(&run))
            .await?;
        Ok(json!({"mode": config::mode_name(mode)}))
    }

    pub(super) async fn sine_hold(&self, conn: &Conn, on: bool) -> Result<Value, MethodError> {
        let run = self.controlled_sine(conn).await?;
        run.ask(|reply| Cmd::Hold { on, reply }, wait_for(&run))
            .await?;
        Ok(view::state_json(Some(&run.view())))
    }

    /// The refusal of a sweep verb outside a sweep, without asking the run thread.
    fn sine_sweep_only(run: &SineRun, verb: &str) -> Result<(), MethodError> {
        if run.wire.test.is_sweep() {
            Ok(())
        } else {
            Err(srun::not_a_sweep(run.wire.test.kind(), verb))
        }
    }

    pub(super) async fn sine_reverse(&self, conn: &Conn) -> Result<Value, MethodError> {
        let run = self.controlled_sine(conn).await?;
        Self::sine_sweep_only(&run, "Reversing the sweep")?;
        run.ask(|reply| Cmd::Reverse { reply }, wait_for(&run))
            .await?;
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn sine_jog(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let delta_hz = protocol::named_f64(params, "delta_hz")?.ok_or_else(|| {
            MethodError::bad_params("delta_hz", "A jog moves the frequency by \"delta_hz\".")
        })?;
        let run = self.controlled_sine(conn).await?;
        Self::sine_sweep_only(&run, "A jog")?;
        let f = run
            .ask(|reply| Cmd::Jog { delta_hz, reply }, wait_for(&run))
            .await?;
        Ok(json!({"frequency_hz": num(config::round(f, 3))}))
    }

    pub(super) async fn sine_goto(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let hz = protocol::named_f64(params, "hz")?.ok_or_else(|| {
            MethodError::bad_params("hz", "A go to moves the frequency to \"hz\".")
        })?;
        let run = self.controlled_sine(conn).await?;
        Self::sine_sweep_only(&run, "A go to")?;
        let band = run.wire.test.band();
        if !(hz >= band.0 && hz <= band.1) {
            return Err(MethodError::named(
                ErrorCode::Refused,
                "out_of_band",
                format!(
                    "{} Hz is outside the band of {} to {} Hz.",
                    num(hz),
                    num(band.0),
                    num(band.1)
                ),
            )
            .with_detail(json!({"band_hz": [num(band.0), num(band.1)]})));
        }
        let f = run
            .ask(|reply| Cmd::GoTo { hz, reply }, wait_for(&run))
            .await?;
        Ok(json!({"frequency_hz": num(config::round(f, 3))}))
    }

    pub(super) async fn sine_adjust(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let key = protocol::named_str(params, "key")?
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                MethodError::bad_params("key", "An adjustment names the parameter's \"key\".")
            })?
            .to_owned();
        let value = protocol::named_f64(params, "value")?.ok_or_else(|| {
            MethodError::bad_params(
                "value",
                "An adjustment gives the parameter's new \"value\".",
            )
        })?;
        let run = self.controlled_sine(conn).await?;
        Self::sine_adjust_check(&run.law, &key, value)?;
        let k = key.clone();
        let v = run
            .ask(
                |reply| Cmd::Adjust {
                    key: k,
                    value,
                    reply,
                },
                wait_for(&run),
            )
            .await?;
        Ok(json!({"key": key, "value": num(v)}))
    }

    /// The refusals of an adjustment the law's declaration settles: an unknown key,
    /// a parameter that is not live, a value out of its range.
    fn sine_adjust_check(facts: &LawFacts, key: &str, value: f64) -> Result<(), MethodError> {
        let Some(spec) = facts.params.iter().find(|p| p.key == key) else {
            return Err(srun::adjust_refused(
                key,
                "unknown_key",
                format!("{} declares no parameter \"{key}\".", facts.name),
            ));
        };
        if !facts.live.iter().any(|k| k == key) {
            return Err(srun::adjust_refused(
                key,
                "not_live",
                format!(
                    "{}'s {} is fixed for the run; only its live parameters change while it \
                     runs.",
                    facts.name, spec.label
                ),
            ));
        }
        if let Some(why) = crate::shock::config::param_problem(spec, &Value::from(value)) {
            return Err(srun::adjust_refused(
                key,
                "out_of_range",
                format!("{}'s {} {why}", facts.name, spec.label),
            ));
        }
        Ok(())
    }

    /// `sine.stop` and `sine.abort`: open to every connection.
    pub(super) async fn sine_end(&self, conn: &Conn, abort: bool) -> Result<Value, MethodError> {
        let (run, by) = {
            let inner = self.inner.lock().await;
            let Some(run) = inner.sine.clone() else {
                return Ok(view::state_json(None));
            };
            (run, inner.session.client_name(conn.id, &conn.peer))
        };
        if !run.view().state.is_live() {
            return Ok(view::state_json(Some(&run.view())));
        }
        stop_sine(
            &run,
            if abort { None } else { Some("operator_stop") },
            by,
            true,
        )
        .await;
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn sine_reset(&self, conn: &Conn) -> Result<Value, MethodError> {
        let (raw, busy) = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.sine.clone().ok_or_else(not_configured)?;
            (run.config.clone(), run.view().state.is_live())
        };
        if busy {
            self.inner.lock().await.run_active()?;
        }
        let (w, report, derived, facts) = self.sine_check(&raw).await?;
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.run_active()?;
        let (Some(derived), Some(facts)) = (derived, facts) else {
            return Err(config::config_invalid(&report));
        };
        let run = self.new_sine_run(&mut inner, w, &derived, facts)?;
        let v = run.view();
        Ok(view::state_json(Some(&v)))
    }

    pub(super) async fn sine_clear(&self, conn: &Conn) -> Result<Value, MethodError> {
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        if inner.sine_live() {
            inner.run_active()?;
        }
        inner.sine = None;
        let state = view::state_json(None);
        inner.sine_state_key = view::state_key(None);
        self.emit("sine.state", state.clone());
        Ok(state)
    }

    pub(super) async fn sine_state_now(&self) -> Value {
        let inner = self.inner.lock().await;
        let v = inner.sine.as_ref().map(|r| r.view());
        view::state_json(v.as_deref())
    }

    pub(super) async fn sine_log(&self, params: &Map<String, Value>) -> Result<Value, MethodError> {
        let since = match params.get("since") {
            None | Some(Value::Null) => 0,
            Some(v) => v.as_u64().ok_or_else(|| {
                MethodError::bad_params("since", "\"since\" is a whole number from 0")
            })?,
        };
        let inner = self.inner.lock().await;
        let (entries, next) = match &inner.sine {
            Some(r) => r.log_since(since),
            None => (Vec::new(), 0),
        };
        Ok(json!({
            "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
            "next": next,
        }))
    }

    /// `sine.subscribe`: replace `conn`'s subscription, and send it the run's latest
    /// data at once when the run is streaming.
    pub(super) async fn sine_subscribe(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let sub = subscribe::parse(params)?.map(Arc::new);
        let result = subscribe::result_json(sub.as_deref());
        match &sub {
            None => {
                self.sine_displays().remove(&conn.id);
            }
            Some(sub) => {
                self.sine_displays().insert(
                    conn.id,
                    SineDisplay {
                        sub: sub.clone(),
                        last: None,
                    },
                );
                let run = self.inner.lock().await.sine.clone();
                if let Some(run) = run.filter(|r| r.view().streaming) {
                    self.emit_sine_data(conn.id, &run, sub);
                    if let Some(d) = self.sine_displays().get_mut(&conn.id) {
                        d.last = Some(Instant::now());
                    }
                }
            }
        }
        Ok(result)
    }

    fn emit_sine_data(&self, conn: u64, run: &SineRun, sub: &Subscription) {
        let v = run.view();
        let mut arrays = Arrays::new();
        let data = subscribe::data_json(&v, sub, &mut arrays);
        let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match codec::encode(&protocol::event("sine.data", seq, data), &arrays) {
            Ok(frame) => {
                let _ = self.events.send(Arc::new(Out::To(conn, Feed::Sine, frame)));
            }
            Err(e) => eprintln!("sds-control: sine.data frame not encoded: {e}"),
        }
    }

    /// Each subscribed connection whose frame is due (every one when `last`).
    fn sine_data_due(&self, run: &SineRun, last: bool) {
        let now = Instant::now();
        let due: Vec<(u64, Arc<Subscription>)> = {
            let mut displays = self.sine_displays();
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
            self.emit_sine_data(conn, run, &sub);
        }
    }

    /// A run published: `sine.state` when what it shows changed, and its new log
    /// entries.
    pub(super) async fn sine_changed(&self, run_id: &str) {
        let mut inner = self.inner.lock().await;
        let Some(run) = inner.sine.clone() else {
            return;
        };
        if run.id != run_id {
            return;
        }
        let shown = run.view();
        let key = view::state_key(Some(&shown));
        if key != inner.sine_state_key {
            inner.sine_state_key = key;
            self.emit("sine.state", view::state_json(Some(&shown)));
        }
        let since = if inner.sine_log_sent.0 == run.id {
            inner.sine_log_sent.1
        } else {
            0
        };
        let (entries, next) = run.log_since(since);
        inner.sine_log_sent = (run.id.clone(), next);
        if !entries.is_empty() {
            self.emit(
                "sine.log",
                json!({
                    "run_id": run.id,
                    "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
                }),
            );
        }
    }

    /// Publish `run`'s `sine.live` five times a second while it streams, and each
    /// subscription's `sine.data` at its own rate, until it has finished; then once
    /// more with the final state. One task per run.
    fn spawn_sine_live(self: &Arc<Self>, run: Arc<SineRun>) {
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
                    .sine
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
                    match codec::encode(&protocol::event("sine.live", seq, data), &arrays) {
                        Ok(frame) => {
                            let _ = svc
                                .events
                                .send(Arc::new(Out::DefaultLive(Feed::Sine, frame)));
                        }
                        Err(e) => eprintln!("sds-control: sine.live frame not encoded: {e}"),
                    }
                }
                svc.sine_data_due(&run, last);
                if last {
                    break;
                }
            }
        });
    }
}

/// End `run`: the controlled stop with `outcome` (`None`: the abort), raised on the
/// run's signals first so identification ends at its next block, then queued.
pub(super) async fn stop_sine(
    run: &SineRun,
    outcome: Option<&'static str>,
    by: String,
    wait: bool,
) {
    if !run.view().state.is_live() {
        return;
    }
    run.shared.signal_end(outcome.is_none(), &by);
    let w = wait_for(run);
    match outcome {
        None => {
            let _ = run.ask(|reply| Cmd::Abort { by, reply }, w).await;
        }
        Some(outcome) if wait => {
            let _ = run
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
            let _ = run.send(Cmd::Stop {
                outcome,
                by,
                reply: None,
            });
        }
    }
}

impl Service {
    /// The catalog id of the Sine law `laws.check` names, by its `id` or the `path`
    /// of its file (imported first, for the controller only), or `None` when it
    /// names no Sine law.
    pub(super) async fn sine_law_id(
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
            if id == laws::SINE_BUILTIN_ID {
                return Ok(Some(id));
            }
            Ok(cat
                .entry(&id)
                .filter(|e| e.world == LawWorld::Sine)
                .map(|e| e.id))
        })
        .await
        .ok()
        .flatten()
    }

    /// `laws.check` of a Sine law: every case of the Sine check on its simulated
    /// station, one `laws.check` event as each starts and as it ends.
    pub(super) async fn sine_laws_check(
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
                let facts = laws::sine_lookup(Some(cat), &law_id)
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
                let choice = hertzforge_random_laws::LawChoice::new(
                    law_id.clone(),
                    crate::mimo::adapter::params_of(&facts.params, &given),
                );
                let factory = hertzforge_sine_laws::resolve_sine(cat, &choice).map_err(|e| {
                    MethodError::named(
                        ErrorCode::Refused,
                        "law_check_unsupported",
                        format!("The Sine check cannot run \"{law_id}\": {e}."),
                    )
                })?;
                Ok((facts, factory))
            })
            .await?;
        let _slot = CheckSlot::take()?;
        let svc = self.clone();
        let check_id = facts.id.clone();
        let results = tokio::task::spawn_blocking(move || {
            crate::sine::law_check::run_check(&check_id, factory, &mut |event| {
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
                    Ok(cat
                        .entry(&id)
                        .map(|e| laws::entry_json(&e))
                        .unwrap_or_else(|| laws::entry_json(&laws::builtin_sine_entry())))
                })
                .await?;
            (law, false)
        } else {
            let sha = facts.sha256.clone();
            let verdicts: Vec<_> = results.into_iter().map(|(_, v)| v).collect();
            let law = self
                .with_catalog(move |cat| {
                    let record = crate::sine::law_check::check_record(sha, now_rfc3339(), verdicts);
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
            "report": crate::sine::law_check::report_json(&cases, passed, recorded),
        }))
    }
}

/// How long the shutdown waits for a Sine run's controlled stop before it aborts:
/// at least ten seconds, three of its batches and twice its stop.
pub(super) fn shutdown_patience(run: &SineRun) -> Duration {
    let v = run.view();
    let tick_s = v.grid.tick_s();
    let batch_s = (tick_s * 2.0).max(0.05);
    let safe_s = run
        .derived
        .get("safe_stop_s")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    Duration::from_secs_f64(10f64.max(3.0 * batch_s).max(2.0 * safe_s) + 1.0)
}
