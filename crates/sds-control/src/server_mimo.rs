//! The MIMO Random and control-law methods of the [`Service`] (schema/PROTOCOL.md,
//! "MIMO Random" and "Control Laws"): a MIMO setup and its run beside the Random
//! slot, under the same run exclusion and the same controller rules.

use super::{Conn, Service, LIVE_TICK};
use crate::laws;
use crate::mimo::driver::{LawRef, StartContext, StopOutcome};
use crate::mimo::setup::{self, SetupContext};
use crate::mimo::sim::{SimClock, SimulatedMimoDriver};
use crate::mimo::{view as mview, MimoRun};
use crate::protocol::{self, ErrorCode, MethodError};
use crate::random::config::Grid;
use crate::random::view::{State, StationRef};
use crate::station::{self, Selection};
use crate::table::{self, Report};
use hertzforge_random_laws::LawCatalog;
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// One connection's MIMO display subscription (filled in by the display module).
pub struct MimoDisplay;

impl Service {
    pub(super) fn mimo_displays(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<u64, MimoDisplay>> {
        match self.mimo_displays.lock() {
            Ok(d) => d,
            Err(p) => p.into_inner(),
        }
    }

    /// Run `f` on the law catalog, opening it on first use.
    async fn with_catalog<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut LawCatalog) -> Result<T, MethodError> + Send + 'static,
    ) -> Result<T, MethodError> {
        let cell = self.catalog.clone();
        let root = self.opts.data_root.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = match cell.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            if guard.is_none() {
                *guard = Some(LawCatalog::open(&root).map_err(|e| laws::catalog_unavailable(&e))?);
            }
            match guard.as_mut() {
                Some(c) => f(c),
                None => Err(MethodError::internal("the law catalog did not open")),
            }
        })
        .await
        .map_err(|e| MethodError::internal(format!("the law catalog task failed: {e}")))?
    }

    fn new_driver(&self, ctx: &StartContext) -> Box<dyn crate::mimo::driver::MimoRunDriver> {
        match &self.opts.mimo_driver {
            Some(f) => f(ctx),
            None => Box::new(SimulatedMimoDriver::new(SimClock::wall(
                self.opts.sim_speed,
            ))),
        }
    }

    /// Parse `setup` and validate it with a fresh driver: the setup, its driver
    /// (configured) and the validation report.
    async fn mimo_check(
        &self,
        setup: &Value,
    ) -> Result<
        (
            Arc<setup::MimoSetup>,
            Box<dyn crate::mimo::driver::MimoRunDriver>,
            crate::mimo::check::Validation,
            StartContext,
        ),
        MethodError,
    > {
        let (table, kind, class, table_report) = {
            let inner = self.inner.lock().await;
            let kind = match inner.stations.selected() {
                Selection::Sim => "sim",
                Selection::Ni { .. } => "ni",
            };
            (
                inner.table.table.clone(),
                kind,
                station::class_name(inner.stations.class()),
                table::validate(&inner.table.table, &inner.ctx()),
            )
        };
        let raw = setup.clone();
        let (parsed, items, law) = self
            .with_catalog(move |cat| {
                let lookup = |id: &str| cat.entry(id).map(|e| laws::law_info(&e));
                let ctx = SetupContext {
                    table: &table,
                    selected_kind: kind,
                    law: &lookup,
                };
                let (s, items) = setup::parse(&raw, &ctx)?;
                let law = lookup(&s.law.id)
                    .filter(|l| l.mimo.is_some())
                    .map(|l| laws::law_ref(&l));
                Ok((s, items, law))
            })
            .await?;
        let mut all = Report::default();
        all.extend(table_report);
        all.extend(items);
        let ctx = StartContext { class, kind };
        let parsed = Arc::new(parsed);
        let mut driver = self.new_driver(&ctx);
        let v = driver.configure(parsed.clone(), law, all);
        Ok((parsed, driver, v, ctx))
    }

    pub(super) async fn mimo_setup(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let setup = params
            .get("setup")
            .filter(|s| s.is_object())
            .ok_or_else(|| {
                MethodError::bad_params("setup", "Setting up MIMO Random needs a setup.")
            })?;
        let dry_run = protocol::param_bool(params, "dry_run")?.unwrap_or(false);
        let (parsed, driver, v, _) = self.mimo_check(setup).await?;
        let validation = v.to_json();
        if dry_run {
            return Ok(json!({ "validation": validation }));
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
        if !v.ok() {
            return Err(setup::setup_invalid(&validation, v.report.errors.len()));
        }
        inner.mimo_runs += 1;
        let id = format!("m-{:04}", inner.mimo_runs);
        let station = StationRef {
            kind: match inner.stations.selected() {
                Selection::Sim => "sim",
                Selection::Ni { .. } => "ni",
            },
            class: station::class_name(inner.stations.class()),
            devices: match inner.stations.selected() {
                Selection::Sim => Vec::new(),
                Selection::Ni { devices } => devices
                    .iter()
                    .map(|d| {
                        let sn = inner
                            .stations
                            .devices()
                            .iter()
                            .find(|x| &x.id == d)
                            .map(|x| x.serial.clone())
                            .unwrap_or_default();
                        (d.clone(), sn)
                    })
                    .collect(),
            },
        };
        let grid = Grid {
            fs_hz: parsed.estimation.fs_hz,
            n_pts: parsed.estimation.n_pts,
            df_hz: parsed.df_hz(),
            lines: parsed.lines_hz().len(),
            loop_s: parsed.loop_s(),
            actual: false,
        };
        let run = Arc::new(MimoRun::new(
            id.clone(),
            parsed,
            station,
            grid,
            validation.clone(),
            driver,
        ));
        inner.mimo = Some(run.clone());
        drop(inner);
        let (_, state) = run.status();
        self.emit("mimo_random.status", state.clone());
        self.emit(
            "mimo_random.validate",
            json!({"run_id": id, "validation": validation}),
        );
        Ok(json!({"state": state, "validation": validation}))
    }

    pub(super) async fn mimo_validate(
        &self,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        match params.get("setup") {
            None | Some(Value::Null) => {
                let inner = self.inner.lock().await;
                Ok(json!({
                    "validation": inner.mimo.as_ref().map_or(Value::Null, |m| m.validation.clone())
                }))
            }
            Some(s) if s.is_object() => {
                let (_, _, v, _) = self.mimo_check(s).await?;
                Ok(json!({"validation": v.to_json()}))
            }
            Some(_) => Err(MethodError::bad_params(
                "setup",
                "\"setup\" is a MIMO setup, or absent for the present one's report.",
            )),
        }
    }

    async fn controlled_mimo(&self, conn: &Conn) -> Result<Arc<MimoRun>, MethodError> {
        let inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.mimo.clone().ok_or_else(|| {
            MethodError::named(
                ErrorCode::Refused,
                "not_configured",
                "MIMO Random is not set up; send a setup first.",
            )
        })
    }

    pub(super) async fn mimo_start(self: &Arc<Self>, conn: &Conn) -> Result<Value, MethodError> {
        let (run, ctx) = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.mimo.clone().ok_or_else(|| {
                MethodError::named(
                    ErrorCode::Refused,
                    "not_configured",
                    "MIMO Random is not set up; send a setup first.",
                )
            })?;
            let state = run.with(|d| d.state());
            if state == State::Finished {
                return Err(MethodError::named(
                    ErrorCode::Refused,
                    "run_finished",
                    "This MIMO run has finished; send the setup again to run.",
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
            let report = table::validate(&inner.table.table, &inner.ctx());
            if !report.errors.is_empty() {
                return Err(super::table_invalid(
                    &report,
                    "on this station, so the run cannot start",
                ));
            }
            let ctx = StartContext {
                class: station::class_name(inner.stations.class()),
                kind: match inner.stations.selected() {
                    Selection::Sim => "sim",
                    Selection::Ni { .. } => "ni",
                },
            };
            (run, ctx)
        };
        run.with(|d| d.start(&ctx))?;
        self.spawn_mimo_watch(run.clone());
        Ok(run.status().1)
    }

    pub(super) async fn mimo_proceed(&self, conn: &Conn) -> Result<Value, MethodError> {
        let run = self.controlled_mimo(conn).await?;
        run.with(|d| d.proceed())?;
        Ok(run.status().1)
    }

    pub(super) async fn mimo_override(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let ack = match params.get("ack") {
            Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_owned(),
            _ => {
                return Err(MethodError::bad_params(
                    "ack",
                    "An override is the operator's own words on why the run should engage; it cannot be empty.",
                ))
            }
        };
        let run = self.controlled_mimo(conn).await?;
        let by = {
            let inner = self.inner.lock().await;
            inner.session.client_name(conn.id, &conn.peer)
        };
        run.with(|d| d.override_gate(&ack, &by))?;
        Ok(run.status().1)
    }

    pub(super) async fn mimo_iterate(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let run = self.controlled_mimo(conn).await?;
        let law = match params.get("law") {
            None | Some(Value::Null) => None,
            Some(Value::Object(m)) => {
                let id = m
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| MethodError::bad_params("law.id", "A law is named by its id."))?
                    .to_owned();
                let p = match m.get("params") {
                    None | Some(Value::Null) => Map::new(),
                    Some(Value::Object(p)) => p.clone(),
                    Some(_) => {
                        return Err(MethodError::bad_params(
                            "law.params",
                            "\"params\" is an object of parameter values.",
                        ))
                    }
                };
                let shape = run.setup.shape();
                let n = run.setup.controlled();
                let (lid, lp) = (id.clone(), p.clone());
                let (info, report) = self
                    .with_catalog(move |cat| {
                        let info = cat.entry(&lid).map(|e| laws::law_info(&e));
                        let mut r = Report::default();
                        setup::check_law(
                            &setup::LawSetup {
                                id: lid,
                                params: lp,
                            },
                            info.clone(),
                            shape,
                            n,
                            &mut r,
                        );
                        Ok((info, r))
                    })
                    .await?;
                if let Some(first) = report.errors.first() {
                    return Err(MethodError::bad_params("law", first.message.clone()));
                }
                let info = info.ok_or_else(|| MethodError::bad_params("law.id", "No such law."))?;
                let law: LawRef = laws::law_ref(&info);
                Some((law, p))
            }
            Some(_) => {
                return Err(MethodError::bad_params(
                    "law",
                    "\"law\" is {\"id\", \"params\"}, or absent for the run's own.",
                ))
            }
        };
        let p = run.with(|d| d.iterate_prediction(law))?;
        Ok(json!({"prediction": mview::prediction_json(&p)}))
    }

    /// `mimo_random.stop` and `mimo_random.abort`: open to every connection.
    pub(super) async fn mimo_end(&self, conn: &Conn, abort: bool) -> Result<Value, MethodError> {
        let (run, by) = {
            let inner = self.inner.lock().await;
            let Some(run) = inner.mimo.clone() else {
                return Ok(mview::status_json(None));
            };
            (run, inner.session.client_name(conn.id, &conn.peer))
        };
        if run.is_live() {
            if abort {
                run.with(|d| d.abort(&by));
            } else {
                run.with(|d| d.stop(StopOutcome::OperatorStop, &by));
            }
        }
        Ok(run.status().1)
    }

    pub(super) async fn mimo_status_now(&self) -> Value {
        let run = self.inner.lock().await.mimo.clone();
        match run {
            Some(r) => r.status().1,
            None => mview::status_json(None),
        }
    }

    pub(super) async fn mimo_report(&self) -> Value {
        let run = self.inner.lock().await.mimo.clone();
        json!({"report": run.and_then(|r| r.report()).unwrap_or(Value::Null)})
    }

    pub(super) async fn laws_list(
        &self,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let world = laws::parse_world(params)?;
        self.with_catalog(move |cat| {
            cat.rescan().map_err(|e| laws::catalog_unavailable(&e))?;
            Ok(laws::list_json(cat, world))
        })
        .await
    }

    pub(super) async fn laws_import(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let path = protocol::named_str(params, "path")?
            .ok_or_else(|| {
                MethodError::bad_params("path", "Importing a law needs the path of its file.")
            })?
            .to_owned();
        self.inner
            .lock()
            .await
            .session
            .require_controller(conn.id)?;
        self.with_catalog(move |cat| laws::import(cat, &path)).await
    }

    /// Follow `run` until it has finished: `mimo_random.status` on every change, and
    /// the report once at the end.
    fn spawn_mimo_watch(self: &Arc<Self>, run: Arc<MimoRun>) {
        let svc = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(LIVE_TICK);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut key = String::new();
            loop {
                tick.tick().await;
                let current = svc
                    .inner
                    .lock()
                    .await
                    .mimo
                    .as_ref()
                    .is_some_and(|m| Arc::ptr_eq(m, &run));
                let (s, state) = run.status();
                let k = mview::status_key(Some(&s));
                if k != key {
                    key = k;
                    svc.emit("mimo_random.status", state);
                }
                if s.state == State::Finished {
                    if let Some(report) = run.report() {
                        svc.emit("mimo_random.report", json!({"report": report}));
                    }
                    break;
                }
                if !current {
                    break;
                }
            }
        });
    }
}
