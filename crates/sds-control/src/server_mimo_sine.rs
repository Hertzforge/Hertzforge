//! The MIMO Sine methods of the [`Service`] (schema/PROTOCOL.md, "MIMO Sine"):
//! a configuration and its run beside the other modules' slots, under the same
//! run exclusion and the same controller rules.

use super::{Conn, Feed, Out, Service, DATA_SLACK, DISPLAY_PERIOD, LIVE_TICK};
use crate::codec::{self, Arrays};
use crate::mimo_sine::config::{self, CheckContext, WireMsine};
use crate::mimo_sine::run::{self as mrun, ArmContext, Cmd, RunPlan, RunShared};
use crate::mimo_sine::subscribe::{self, Subscription};
use crate::mimo_sine::view::{self, MsView};
use crate::mimo_sine::{laws as mlaws, MsineRun};
use crate::protocol::{self, num, ErrorCode, MethodError};
use crate::random::run::LevelTarget;
use crate::random::view::State;
use crate::sine::view::StationRef;
use crate::station::{self, Selection, StationPlan};
use crate::table;
use crate::views::{self as vk, Axis, Role, Scale, Series};
use hertzforge_mimo_sine::{LevelMode, MimoSineLawFactory};
use hertzforge_random_laws::LawWorld;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One connection's MIMO Sine subscription and when it was last sent a frame.
pub struct MsineDisplay {
    sub: Arc<Subscription>,
    last: Option<Instant>,
}

/// How long to wait for the run thread to answer: its identification, a margin.
fn wait_for(run: &MsineRun) -> Duration {
    let fs = run.wire.fs_hz().max(1.0);
    Duration::from_secs_f64(30.0 + 2.0 * run.wire.identification_s(fs))
}

const ARM_WAIT: Duration = Duration::from_secs(120);

/// The longest a law file's constructor may take when a run engages.
const BUILD_BUDGET: Duration = Duration::from_secs(120);

fn not_configured() -> MethodError {
    MethodError::named(
        ErrorCode::Refused,
        "not_configured",
        "MIMO Sine is not configured; configure a test first.",
    )
}

/// The configuration's views: per control its reference and bands at full
/// level and, with phases, its reference phase; per limit channel its bound.
fn config_views(w: &WireMsine, arrays: &mut Arrays) -> Value {
    let band = w.band();
    let fx = || Axis::new("Frequency", Some("Hz"), Scale::Log);
    let mut out = Vec::new();
    for c in &w.setup.controls {
        let Some(f) = crate::sine::profile::figures(&c.profile, band) else {
            continue;
        };
        let x = || f.freqs_hz.clone();
        out.push(vk::spectrum(
            arrays,
            &format!("Profile {}", c.channel),
            &fx(),
            &Axis::new("Acceleration", Some("g"), Scale::Log),
            vec![
                Series::new("Profile", Role::Reference, x(), f.target.clone()),
                Series::new("Alarm high", Role::Tolerance, x(), f.alarm_hi.clone()),
                Series::new("Alarm low", Role::Tolerance, x(), f.alarm_lo.clone()),
                Series::new("Abort high", Role::Abort, x(), f.abort_hi.clone()),
                Series::new("Abort low", Role::Abort, x(), f.abort_lo.clone()),
            ],
            Some("frequency"),
        ));
        if w.setup.phase_specified {
            let mut r = vec![hertzforge_sdm::C64::new(0.0, 0.0); w.setup.controls.len()];
            let k = w
                .setup
                .controls
                .iter()
                .position(|x| x.channel == c.channel)
                .unwrap_or(0);
            let ph: Vec<f64> = f
                .freqs_hz
                .iter()
                .map(|fr| {
                    hertzforge_mimo_sine::core::reference_at(&w.setup, *fr, &mut r);
                    r[k].arg().to_degrees()
                })
                .collect();
            out.push(vk::spectrum(
                arrays,
                &format!("Phase {}", c.channel),
                &fx(),
                &Axis::new("Phase", Some("deg"), Scale::Linear),
                vec![Series::new("Reference phase", Role::Reference, x(), ph)],
                Some("frequency"),
            ));
        }
    }
    for l in &w.setup.limits {
        let Some(f) = crate::sine::profile::figures(&l.profile, band) else {
            continue;
        };
        let x = || f.freqs_hz.clone();
        out.push(vk::spectrum(
            arrays,
            &format!("Limit {}", l.channel),
            &fx(),
            &Axis::new("Acceleration", Some("g"), Scale::Log),
            vec![
                Series::new("Bound", Role::Limit, x(), f.target.clone()),
                Series::new("Abort high", Role::Abort, x(), f.abort_hi.clone()),
            ],
            Some("frequency"),
        ));
    }
    Value::Array(out)
}

impl Service {
    pub(super) fn mimo_sine_displays(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<u64, MsineDisplay>> {
        match self.mimo_sine_displays.lock() {
            Ok(d) => d,
            Err(p) => p.into_inner(),
        }
    }

    fn mimo_sine_station_ref(inner: &super::Inner) -> StationRef {
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

    /// Parse and check `config` against the present table and station.
    async fn mimo_sine_check(
        &self,
        config: &Value,
    ) -> Result<
        (
            WireMsine,
            table::Report,
            Option<config::Derived>,
            Option<mlaws::LawFacts>,
        ),
        MethodError,
    > {
        let w = config::parse(config)?;
        let (tbl, table_report, sim) = {
            let inner = self.inner.lock().await;
            (
                inner.table.table.clone(),
                table::validate(&inner.table.table, &inner.ctx()),
                *inner.stations.selected() == Selection::Sim,
            )
        };
        let timeout_s = self.opts.timeout.as_secs_f64();
        let id = w.setup.law.id.clone();
        let (w, report, derived, facts) = self
            .with_catalog(move |cat| {
                let lookup = |i: &str| mlaws::facts(cat, i);
                let ctx = CheckContext {
                    table: &tbl,
                    law: &lookup,
                    timeout_s,
                    sim,
                };
                let (report, derived) = w.check(&ctx);
                let facts = mlaws::facts(cat, &id);
                Ok((w, report, derived, facts))
            })
            .await?;
        let mut all = table_report;
        all.extend(report);
        let derived = if all.errors.is_empty() { derived } else { None };
        Ok((w, all, derived, facts))
    }

    pub(super) async fn mimo_sine_configure(
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
        let (w, report, derived, facts) = self.mimo_sine_check(config).await?;
        if dry_run {
            let mut arrays = Arrays::new();
            let views = match &derived {
                Some(_) => config_views(&w, &mut arrays),
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
            return Err(crate::sine::config::config_invalid(&report));
        };
        let run = self.new_mimo_sine_run(&mut inner, w, &derived, facts)?;
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
    fn new_mimo_sine_run(
        &self,
        inner: &mut super::Inner,
        w: WireMsine,
        derived: &config::Derived,
        facts: mlaws::LawFacts,
    ) -> Result<Arc<MsineRun>, MethodError> {
        inner.mimo_sine_runs += 1;
        let run_id = format!("msine-{:04}", inner.mimo_sine_runs);
        let station = Self::mimo_sine_station_ref(inner);
        let law_values: Vec<(String, String, String)> = facts
            .values
            .iter()
            .map(|d| (d.key.clone(), d.label.clone(), d.unit.clone()))
            .collect();
        let law = crate::mimo_sine::law_state(&facts, &w.setup.law.params);
        let initial = mrun::configured_view(&run_id, &w, derived, station, law, law_values);
        let shared = Arc::new(RunShared::new(
            initial.clone(),
            self.mimo_sine_notify.clone(),
        ));
        let raw = w.raw.clone();
        let wire = Arc::new(w);
        let plan = RunPlan {
            run_id: run_id.clone(),
            config: wire.clone(),
            law: facts.clone(),
            sim_speed: self.opts.sim_speed,
            capture_root: Some(self.opts.records_root()),
            timeout_s: self.opts.timeout.as_secs_f64(),
            record: self.opts.record.clone(),
            derived: derived.to_json(),
            structure: derived.structure.clone(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        mrun::spawn(plan, initial.clone(), shared.clone(), rx)
            .map_err(|e| MethodError::internal(format!("could not start the run thread: {e}")))?;
        let run = Arc::new(MsineRun::new(
            run_id.clone(),
            shared.clone(),
            tx,
            raw,
            derived.to_json(),
            wire.clone(),
            facts,
        ));
        inner.mimo_sine = Some(run.clone());
        shared.add_log(
            "state",
            format!("configured \"{}\" on {}", wire.title, initial.station.class),
        );
        let _ = self.mimo_sine_notify.send((run_id, Arc::new(initial)));
        Ok(run)
    }

    pub(super) async fn mimo_sine_config(&self) -> (Value, Arrays) {
        let inner = self.inner.lock().await;
        let mut arrays = Arrays::new();
        let result = match &inner.mimo_sine {
            None => json!({"run_id": null, "config": null, "derived": null, "views": []}),
            Some(run) => json!({
                "run_id": run.id,
                "config": run.config,
                "derived": run.derived,
                "views": config_views(&run.wire, &mut arrays),
            }),
        };
        (result, arrays)
    }

    async fn controlled_mimo_sine(&self, conn: &Conn) -> Result<Arc<MsineRun>, MethodError> {
        let inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.mimo_sine.clone().ok_or_else(not_configured)
    }

    /// What a run arms on now: the table, the selected station and its law.
    async fn mimo_sine_arm_context(&self, run: &MsineRun) -> Result<ArmContext, MethodError> {
        let (tbl, station_plan, hardware, revision, devices) = {
            let inner = self.inner.lock().await;
            inner.table.arm_gate()?;
            let report = table::validate(&inner.table.table, &inner.ctx());
            if !report.errors.is_empty() {
                return Err(super::table_invalid(
                    &report,
                    "on this station, so the run cannot arm",
                ));
            }
            let fs_hz = run.wire.fs_hz();
            let (plan, hardware) = match inner.stations.selected() {
                Selection::Sim => (sim_plan(&run.wire, &inner.table.table, fs_hz)?, false),
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
                inner.table.table.clone(),
                plan,
                hardware,
                inner.table.revision,
                inner.stations.devices().to_vec(),
            )
        };
        let id = run.wire.setup.law.id.clone();
        let given = run.wire.setup.law.params.clone();
        let budget = Duration::from_secs_f64(
            (run.wire.setup.guards.law_budget * run.wire.tick_s()).max(1e-4),
        );
        let law: Result<Arc<dyn MimoSineLawFactory>, MethodError> = self
            .with_catalog(move |cat| {
                Ok(mlaws::resolve(
                    cat,
                    &id,
                    &given,
                    budget,
                    BUILD_BUDGET,
                    hardware,
                ))
            })
            .await?;
        // A law without a passed check is refused before anything opens.
        if let Err(e) = &law {
            if e.reason == Some("law_not_checked") {
                return Err(e.clone());
            }
        }
        Ok(ArmContext {
            table: tbl,
            station: station_plan,
            scanner: self.opts.scanner.clone(),
            data_root: self.opts.data_root.clone(),
            bind: self.opts.bind.clone(),
            law,
            revision,
            devices,
        })
    }

    pub(super) async fn mimo_sine_arm(
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
            let run = inner.mimo_sine.clone().ok_or_else(not_configured)?;
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
                || inner.playback_live()
                || inner.modal_test_live()
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
        let ctx = self.mimo_sine_arm_context(&run).await?;
        if start {
            run.ask(|reply| Cmd::Start { ctx, reply }, ARM_WAIT).await?;
        } else {
            run.ask(|reply| Cmd::Arm { ack, ctx, reply }, ARM_WAIT)
                .await?;
        }
        self.spawn_mimo_sine_live(run.clone());
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn mimo_sine_proceed(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let ack = match params.get("acknowledge") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
            Some(_) => {
                return Err(MethodError::bad_params(
                    "acknowledge",
                    "An acknowledgement is the operator's own words; it cannot be empty.",
                ))
            }
        };
        let run = self.controlled_mimo_sine(conn).await?;
        let v = run.view();
        if v.state != State::Review {
            return Err(mrun::wrong_state(&v, "Proceeding"));
        }
        run.ask(|reply| Cmd::Proceed { ack, reply }, wait_for(&run))
            .await?;
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn mimo_sine_level(
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
        let run = self.controlled_mimo_sine(conn).await?;
        let level = run
            .ask(|reply| Cmd::Level { target, reply }, wait_for(&run))
            .await?;
        Ok(json!({"level_db": num(crate::sine::config::round(level, 2))}))
    }

    pub(super) async fn mimo_sine_mode(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let mode = match protocol::named_str(params, "mode")? {
            Some("auto") => LevelMode::Auto,
            Some("manual") => LevelMode::Manual,
            _ => {
                return Err(MethodError::bad_params(
                    "mode",
                    "\"mode\" is \"auto\" or \"manual\"",
                ))
            }
        };
        let run = self.controlled_mimo_sine(conn).await?;
        run.ask(|reply| Cmd::Mode { mode, reply }, wait_for(&run))
            .await?;
        Ok(json!({"mode": config::mode_name(mode)}))
    }

    pub(super) async fn mimo_sine_hold(&self, conn: &Conn, on: bool) -> Result<Value, MethodError> {
        let run = self.controlled_mimo_sine(conn).await?;
        run.ask(|reply| Cmd::Hold { on, reply }, wait_for(&run))
            .await?;
        Ok(view::state_json(Some(&run.view())))
    }

    /// The refusal of a sweep verb outside a sweep, without asking the run thread.
    fn mimo_sine_sweep_only(run: &MsineRun, verb: &str) -> Result<(), MethodError> {
        if run.wire.is_sweep() {
            Ok(())
        } else {
            Err(mrun::not_a_sweep(run.wire.kind(), verb))
        }
    }

    /// `mimo_sine.reverse`: a sweep in this version turns only at the band's
    /// edges.
    pub(super) async fn mimo_sine_reverse(&self, conn: &Conn) -> Result<Value, MethodError> {
        let run = self.controlled_mimo_sine(conn).await?;
        Self::mimo_sine_sweep_only(&run, "Reversing the sweep")?;
        Err(MethodError::named(
            ErrorCode::Refused,
            "reverse_not_available",
            "A MIMO Sine sweep turns only at the band's edges; go to a frequency, or stop and \
             start a sweep in the other direction.",
        ))
    }

    pub(super) async fn mimo_sine_goto(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
        relative: bool,
    ) -> Result<Value, MethodError> {
        let (key, words) = if relative {
            ("delta_hz", "A jog moves the frequency by \"delta_hz\".")
        } else {
            ("hz", "A go to moves the frequency to \"hz\".")
        };
        let hz =
            protocol::named_f64(params, key)?.ok_or_else(|| MethodError::bad_params(key, words))?;
        let run = self.controlled_mimo_sine(conn).await?;
        Self::mimo_sine_sweep_only(&run, if relative { "A jog" } else { "A go to" })?;
        let band = run.wire.band();
        if !(relative || (hz >= band.0 && hz <= band.1)) {
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
            .ask(
                |reply| Cmd::GoTo {
                    hz,
                    relative,
                    reply,
                },
                wait_for(&run),
            )
            .await?;
        Ok(json!({"frequency_hz": num(crate::sine::config::round(f, 3))}))
    }

    /// `mimo_sine.stop` and `mimo_sine.abort`: open to every connection.
    pub(super) async fn mimo_sine_end(
        &self,
        conn: &Conn,
        abort: bool,
    ) -> Result<Value, MethodError> {
        let (run, by) = {
            let inner = self.inner.lock().await;
            let Some(run) = inner.mimo_sine.clone() else {
                return Ok(view::state_json(None));
            };
            (run, inner.session.client_name(conn.id, &conn.peer))
        };
        if !run.view().state.is_live() {
            return Ok(view::state_json(Some(&run.view())));
        }
        stop_mimo_sine(
            &run,
            if abort { None } else { Some("operator_stop") },
            by,
            true,
        )
        .await;
        Ok(view::state_json(Some(&run.view())))
    }

    pub(super) async fn mimo_sine_reset(&self, conn: &Conn) -> Result<Value, MethodError> {
        let (raw, busy) = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.mimo_sine.clone().ok_or_else(not_configured)?;
            (run.config.clone(), run.view().state.is_live())
        };
        if busy {
            self.inner.lock().await.run_active()?;
        }
        let (w, report, derived, facts) = self.mimo_sine_check(&raw).await?;
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.run_active()?;
        let (Some(derived), Some(facts)) = (derived, facts) else {
            return Err(crate::sine::config::config_invalid(&report));
        };
        let run = self.new_mimo_sine_run(&mut inner, w, &derived, facts)?;
        let v = run.view();
        Ok(view::state_json(Some(&v)))
    }

    pub(super) async fn mimo_sine_clear(&self, conn: &Conn) -> Result<Value, MethodError> {
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        if inner.mimo_sine_live() {
            inner.run_active()?;
        }
        inner.mimo_sine = None;
        let state = view::state_json(None);
        inner.mimo_sine_state_key = view::state_key(None);
        self.emit("mimo_sine.state", state.clone());
        Ok(state)
    }

    pub(super) async fn mimo_sine_state_now(&self) -> Value {
        let inner = self.inner.lock().await;
        let v = inner.mimo_sine.as_ref().map(|r| r.view());
        view::state_json(v.as_deref())
    }

    pub(super) async fn mimo_sine_report(&self) -> Value {
        let inner = self.inner.lock().await;
        let report = inner
            .mimo_sine
            .as_ref()
            .and_then(|r| r.view().report.clone())
            .unwrap_or(Value::Null);
        json!({"report": report})
    }

    pub(super) async fn mimo_sine_log(
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
        let (entries, next) = match &inner.mimo_sine {
            Some(r) => r.log_since(since),
            None => (Vec::new(), 0),
        };
        Ok(json!({
            "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
            "next": next,
        }))
    }

    /// `mimo_sine.subscribe`: replace `conn`'s subscription, and send it the
    /// run's latest data at once when the run is streaming.
    pub(super) async fn mimo_sine_subscribe(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let sub = subscribe::parse(params)?.map(Arc::new);
        let result = subscribe::result_json(sub.as_deref());
        match &sub {
            None => {
                self.mimo_sine_displays().remove(&conn.id);
            }
            Some(sub) => {
                self.mimo_sine_displays().insert(
                    conn.id,
                    MsineDisplay {
                        sub: sub.clone(),
                        last: None,
                    },
                );
                let run = self.inner.lock().await.mimo_sine.clone();
                if let Some(run) = run.filter(|r| r.view().streaming) {
                    self.emit_mimo_sine_data(conn.id, &run.view(), sub);
                    if let Some(d) = self.mimo_sine_displays().get_mut(&conn.id) {
                        d.last = Some(Instant::now());
                    }
                }
            }
        }
        Ok(result)
    }

    fn emit_mimo_sine_data(&self, conn: u64, v: &MsView, sub: &Subscription) {
        let mut arrays = Arrays::new();
        let data = subscribe::data_json(v, sub, &mut arrays);
        let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match codec::encode(&protocol::event("mimo_sine.data", seq, data), &arrays) {
            Ok(frame) => {
                let _ = self
                    .events
                    .send(Arc::new(Out::To(conn, Feed::MimoSine, frame)));
            }
            Err(e) => eprintln!("sds-control: mimo_sine.data frame not encoded: {e}"),
        }
    }

    /// Each subscribed connection whose frame is due (every one when `last`).
    fn mimo_sine_data_due(&self, v: &MsView, last: bool) {
        let now = Instant::now();
        let due: Vec<(u64, Arc<Subscription>)> = {
            let mut displays = self.mimo_sine_displays();
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
            self.emit_mimo_sine_data(conn, v, &sub);
        }
    }

    /// A run published: `mimo_sine.state` when what it shows changed, its new
    /// log entries, a finished pass's `mimo_sine.sweep` and the report.
    pub(super) async fn mimo_sine_changed(&self, run_id: &str) {
        let mut inner = self.inner.lock().await;
        let Some(run) = inner.mimo_sine.clone() else {
            return;
        };
        if run.id != run_id {
            return;
        }
        let shown = run.view();
        let key = view::state_key(Some(&shown));
        if key != inner.mimo_sine_state_key {
            inner.mimo_sine_state_key = key;
            self.emit("mimo_sine.state", view::state_json(Some(&shown)));
        }
        let since = if inner.mimo_sine_log_sent.0 == run.id {
            inner.mimo_sine_log_sent.1
        } else {
            0
        };
        let (entries, next) = run.log_since(since);
        inner.mimo_sine_log_sent = (run.id.clone(), next);
        if !entries.is_empty() {
            self.emit(
                "mimo_sine.log",
                json!({
                    "run_id": run.id,
                    "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
                }),
            );
        }
        if let Some(p) = &shown.last_pass {
            let sent =
                inner.mimo_sine_pass_sent.0 == run.id && inner.mimo_sine_pass_sent.1 >= p.pass;
            if !sent {
                inner.mimo_sine_pass_sent = (run.id.clone(), p.pass);
                let mut arrays = Arrays::new();
                let data = view::sweep_json(&shown, p, &mut arrays);
                let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                match codec::encode(&protocol::event("mimo_sine.sweep", seq, data), &arrays) {
                    Ok(frame) => {
                        let _ = self.events.send(Arc::new(Out::Binary(frame)));
                    }
                    Err(e) => eprintln!("sds-control: mimo_sine.sweep frame not encoded: {e}"),
                }
            }
        }
        if let Some(r) = &shown.report {
            if inner.mimo_sine_report_sent != run.id {
                inner.mimo_sine_report_sent = run.id.clone();
                self.emit("mimo_sine.report", json!({"report": r}));
            }
        }
    }

    /// Publish `run`'s `mimo_sine.live` five times a second while it streams,
    /// and each subscription's `mimo_sine.data` at its own rate, until it has
    /// finished; then once more with the final state. One task per run.
    fn spawn_mimo_sine_live(self: &Arc<Self>, run: Arc<MsineRun>) {
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
                    .mimo_sine
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
                    match codec::encode(&protocol::event("mimo_sine.live", seq, data), &arrays) {
                        Ok(frame) => {
                            let _ = svc
                                .events
                                .send(Arc::new(Out::DefaultLive(Feed::MimoSine, frame)));
                        }
                        Err(e) => eprintln!("sds-control: mimo_sine.live frame not encoded: {e}"),
                    }
                }
                svc.mimo_sine_data_due(&v, last);
                if last {
                    break;
                }
            }
        });
    }

    /// The catalog id of the MIMO Sine law `laws.check` names: a law file of
    /// the `sine_mimo` world by its `id` or the `path` of its file (imported
    /// first, for the controller only), or a built-in by its `id` with
    /// `"world": "sine_mimo"` (built-in names are shared with other worlds).
    /// `None` when it names no MIMO Sine law.
    pub(super) async fn mimo_sine_law_id(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Option<String> {
        let id = params.get("id").and_then(Value::as_str).map(str::to_owned);
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let named_world = params.get("world").and_then(Value::as_str) == Some("sine_mimo");
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
            let builtin = named_world && hertzforge_mimo_sine_laws::is_builtin(&id);
            Ok((builtin
                || cat
                    .entry(&id)
                    .is_some_and(|e| e.world == LawWorld::SineMimo))
            .then_some(id))
        })
        .await
        .ok()
        .flatten()
    }

    /// `laws.check` of a MIMO Sine law: every case of the laws crate's check
    /// on the simulator, one `laws.check` event as each starts and as it ends.
    /// A law file's scored verdicts are recorded against its exact file.
    pub(super) async fn mimo_sine_laws_check(
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
                let facts = mlaws::facts(cat, &law_id)
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
                let factory = mlaws::resolve(
                    cat,
                    &law_id,
                    &given,
                    Duration::from_secs(1),
                    BUILD_BUDGET,
                    false,
                )
                .map_err(|e| bad_choice(&law_id, &e.message))?;
                Ok((facts, factory))
            })
            .await?;
        let _slot = CheckSlot::take()?;
        let svc = self.clone();
        let check_id = facts.id.clone();
        let (verdicts, lines) = tokio::task::spawn_blocking(move || {
            crate::mimo_sine::law_check::run_check(&check_id, factory, &mut |event| {
                svc.emit("laws.check", event);
            })
        })
        .await
        .map_err(|e| MethodError::internal(format!("the law check failed: {e}")))??;
        let recorded = !facts.builtin;
        let sha = facts.sha256.clone();
        let v2 = verdicts.clone();
        let id2 = facts.id.clone();
        let law = self
            .with_catalog(move |cat| {
                if recorded {
                    let record =
                        hertzforge_mimo_sine_laws::check::check_record(&sha, &now_rfc3339(), &v2);
                    cat.record_check(&record)
                        .map_err(|e| not_recorded(&e.to_string()))?;
                }
                mlaws::entry_json(cat, &id2)
                    .ok_or_else(|| MethodError::internal("the checked law left the catalog"))
            })
            .await?;
        Ok(json!({
            "law": law,
            "report": crate::mimo_sine::law_check::report_json(&verdicts, lines, recorded),
        }))
    }
}

/// The simulator station for `w` on `table`: its sine structure by name (or
/// the linear one of the table's size), the table its size. A run never arms on
/// the default one-drive simulator.
fn sim_plan(
    w: &WireMsine,
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
    let s = match &w.structure {
        Some(name) => hertzforge_station::SimSineStructure::from_name(name).ok_or_else(|| {
            MethodError::named(
                ErrorCode::Refused,
                "arm_refused",
                format!("The simulator has no structure \"{name}\"."),
            )
        })?,
        None => config::default_structure(inputs, drives).ok_or_else(|| {
            MethodError::named(
                ErrorCode::Refused,
                "arm_refused",
                format!(
                    "The simulator has no sine structure of {inputs} inputs and {drives} drives; \
                     name one in \"simulator.structure\" or size the table to one."
                ),
            )
        })?,
    };
    let (n_in, n_d) = s.dims();
    if (inputs, drives) != (n_in, n_d) {
        return Err(MethodError::named(
            ErrorCode::Refused,
            "arm_refused",
            format!(
                "The simulator's structure \"{}\" has {n_in} inputs and {n_d} drives; the table \
                 has {inputs} enabled inputs and {drives} drives.",
                s.name()
            ),
        )
        .with_detail(json!({"structure": s.name(), "inputs": n_in, "drives": n_d})));
    }
    Ok(StationPlan::Sim {
        fs_hz,
        plant: Some(hertzforge_station::SimPlant::SineStructure(s)),
        outputs: Some(n_d as u32),
    })
}

/// End `run`: the controlled stop with `outcome` (`None`: the abort), raised on
/// the run's signals first so identification ends at its next block, then
/// queued.
pub(super) async fn stop_mimo_sine(
    run: &MsineRun,
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

/// How long the shutdown waits for a MIMO Sine run's controlled stop before it
/// aborts: at least ten seconds and twice its stop.
pub(super) fn shutdown_patience(run: &MsineRun) -> Duration {
    let safe_s = run
        .derived
        .get("safe_stop_s")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    Duration::from_secs_f64(10f64.max(2.0 * safe_s) + 1.0)
}
