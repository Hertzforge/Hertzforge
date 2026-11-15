//! The `playback.*` methods (schema/PROTOCOL.md, "Playback"): configure with the drive
//! file's check, start, stop, abort, reset, clear, the state and the log, and the run's
//! `playback.state`, `playback.log` and `playback.live` events.

use super::{Conn, Out, Service, DATA_SLACK, LIVE_TICK};
use crate::codec::{self, Arrays};
use crate::playback::config::{self, WirePlayback};
use crate::playback::run::{self as prun, PlaybackRun, PlaybackView, State};
use crate::protocol::{self, ErrorCode, MethodError};
use crate::pulse::rails::Shared;
use crate::station::{self, Selection, StationPlan};
use crate::table::{self, Item, Report};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_playback::plan;
use hertzforge_playback::CheckedDrive;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often `playback.live` goes out.
const LIVE_PERIOD: Duration = Duration::from_millis(100);
/// How long the drive file's check may take.
const CHECK_WAIT: Duration = Duration::from_secs(600);

fn not_configured() -> MethodError {
    MethodError::named(
        ErrorCode::Refused,
        "not_configured",
        "Playback is not configured; configure a test first.",
    )
}

/// How long the shutdown waits for the controlled stop before it aborts.
pub(super) fn wait_for(_run: &PlaybackRun) -> Duration {
    Duration::from_secs_f64((2.0 * plan::STOP_RAMP_S).max(10.0))
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

/// The simulator a run arms on: the four-by-four modal plant for a table of four inputs
/// and four drives, else the resonator bank widened to the drive rows.
pub fn sim_plan(fs_hz: f64, table: &ChannelTable) -> StationPlan {
    let inputs = table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role != ChannelRole::Drive)
        .count();
    let drives = config::drive_rows(table).len();
    if inputs == 4 && drives == 4 {
        if let Some(p) = hertzforge_station::SimStandardPlant::from_name("modal-4x4") {
            return StationPlan::Sim {
                fs_hz,
                plant: Some(hertzforge_station::SimPlant::Modal(p)),
                outputs: Some(4),
            };
        }
    }
    StationPlan::Sim {
        fs_hz,
        plant: None,
        outputs: (drives > 1).then_some(drives as u32),
    }
}

/// What a configure found: the configuration, the field and table report, the file and
/// its check when it could be read, the check report and the derived values.
struct Checked {
    w: WirePlayback,
    report: Report,
    drive: Option<(
        hertzforge_playback::DriveFile,
        hertzforge_playback::FileCheck,
    )>,
    check: Value,
    derived: Value,
    table: ChannelTable,
    source: &'static str,
    class: &'static str,
}

impl Service {
    /// Parse `config` and check it, the drive file on a worker thread off the lock.
    async fn playback_check(&self, config: &Value) -> Result<Checked, MethodError> {
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
        let fields = w.check_fields(&tbl);
        report.errors.extend(fields.errors);
        let w2 = w.clone();
        let read = tokio::time::timeout(
            CHECK_WAIT,
            tokio::task::spawn_blocking(move || config::check_drive(&w2)),
        )
        .await
        .map_err(|_| MethodError::internal("the drive file's check did not finish in time"))?
        .map_err(|e| MethodError::internal(format!("the drive file's check failed: {e}")))?;
        let (drive, check, derived) = match read {
            Err(e) => {
                report
                    .errors
                    .push(Item::field(e.reason, "drive", e.message.clone()));
                (
                    None,
                    json!({"pass": false, "refusals": [{"reason": e.reason, "message": e.message,
                           "detail": e.detail}]}),
                    Value::Null,
                )
            }
            Ok((file, chk)) => {
                let setup = w.setup();
                let refusals = plan::refusals(
                    &file,
                    &chk,
                    &setup,
                    &config::drive_rows(&tbl),
                    &config::offer(&tbl),
                );
                report.errors.extend(config::items(&refusals));
                if let Some(i) = config::rate_item(&file) {
                    report.errors.push(i);
                }
                let check = config::report_with_previews(&file, &chk, &setup, &refusals);
                let derived = config::derived(&w, &file);
                (Some((file, chk)), check, derived)
            }
        };
        Ok(Checked {
            w,
            report,
            drive,
            check,
            derived,
            table: tbl,
            source,
            class,
        })
    }

    pub(super) async fn playback_configure(
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
        let c = self.playback_check(config).await?;
        if dry_run {
            return Ok(json!({
                "errors": c.report.errors.iter().map(Item::to_json).collect::<Vec<_>>(),
                "warnings": c.report.warnings.iter().map(Item::to_json).collect::<Vec<_>>(),
                "check": c.check,
                "derived": c.derived,
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
        let Some((file, chk)) = c.drive.clone().filter(|_| c.report.errors.is_empty()) else {
            return Err(config::config_invalid(&c.report, Some(&c.check)));
        };
        let drive = CheckedDrive::new(
            file,
            chk,
            c.w.setup(),
            &config::drive_rows(&c.table),
            &config::offer(&c.table),
        )
        .map_err(|r| {
            let rep = Report {
                errors: config::items(&r),
                ..Report::default()
            };
            config::config_invalid(&rep, Some(&c.check))
        })?;
        let run = self.new_playback_run(&mut inner, c, drive);
        Ok(json!({
            "state": prun::state_json(Some(&run.view())),
            "check": run.check,
            "derived": run.derived,
            "warnings": [],
        }))
    }

    fn new_playback_run(
        &self,
        inner: &mut super::Inner,
        c: Checked,
        drive: CheckedDrive,
    ) -> Arc<PlaybackRun> {
        inner.playback_runs += 1;
        let run_id = format!("playback-{:04}", inner.playback_runs);
        let initial = prun::configured_view(&run_id, &c.w, &drive, &c.table, c.source, c.class);
        let shared = Arc::new(Shared::new(initial.clone(), self.playback_notify.clone()));
        let title = c.w.title.clone();
        let sha = drive.file.sha256.clone();
        let run = Arc::new(PlaybackRun::new(
            run_id.clone(),
            shared.clone(),
            c.w,
            drive,
            c.check,
            c.derived,
        ));
        shared.add_log(
            "state",
            format!("configured \"{title}\" on {}", initial.station_class),
        );
        shared.add_log_data(
            "check",
            "the drive file's check passed",
            Some(json!({"event": "check_passed", "sha256": sha})),
        );
        inner.playback = Some(run.clone());
        let _ = self.playback_notify.send((run_id, Arc::new(initial)));
        run
    }

    pub(super) async fn playback_config(&self) -> Value {
        let inner = self.inner.lock().await;
        match &inner.playback {
            None => json!({"run_id": null, "config": null, "check": null, "derived": null}),
            Some(r) => json!({"run_id": r.id, "config": r.config, "check": r.check,
                              "derived": r.derived}),
        }
    }

    pub(super) async fn playback_start(
        self: &Arc<Self>,
        conn: &Conn,
    ) -> Result<Value, MethodError> {
        let (run, ctx) = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.playback.clone().ok_or_else(not_configured)?;
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
                return Err(prun::refuse(
                    "wrong_state",
                    format!(
                        "playback.start does not apply while the run is {}.",
                        v.state.name()
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
                    "on this station, so the run cannot start",
                ));
            }
            // The table as it is now must still carry the run as it was checked.
            let rows = config::drive_rows(&tbl);
            let mut r = run.wire.check_fields(&tbl);
            let refusals = plan::refusals(
                &run.drive.file,
                &run.drive.check,
                &run.drive.setup,
                &rows,
                &config::offer(&tbl),
            );
            r.errors.extend(config::items(&refusals));
            if rows != run.drive.rows {
                r.errors.push(Item::field(
                    "table_changed",
                    "map",
                    "The channel table's drive rows have changed since the run was configured; \
                     configure it again.",
                ));
            }
            if !r.errors.is_empty() {
                return Err(config::config_invalid(&r, None));
            }
            let fs = run.drive.file.rate_hz;
            let plan = match inner.stations.selected() {
                Selection::Sim => sim_plan(fs, &tbl),
                Selection::Ni { devices } => {
                    let (inputs, outputs) = hertzforge_station::physical_from_table(&tbl);
                    StationPlan::Ni {
                        devices: devices.clone(),
                        inputs,
                        outputs,
                        fs_hz: fs,
                    }
                }
            };
            run.mark_starting();
            let ctx = prun::StartContext {
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
        self.spawn_playback_live(run.clone());
        started?;
        Ok(prun::state_json(Some(&run.view())))
    }

    /// `playback.stop` and `playback.abort`: open to every connection.
    pub(super) async fn playback_end(
        &self,
        conn: &Conn,
        abort: bool,
    ) -> Result<Value, MethodError> {
        let (run, by) = {
            let inner = self.inner.lock().await;
            let Some(run) = inner.playback.clone() else {
                return Ok(prun::state_json(None));
            };
            (run, inner.session.client_name(conn.id, &conn.peer))
        };
        if !run.view().state.is_live() {
            return Ok(prun::state_json(Some(&run.view())));
        }
        if abort {
            run.abort(&by);
        } else {
            run.stop(&by);
        }
        Ok(prun::state_json(Some(&run.view())))
    }

    pub(super) async fn playback_reset(&self, conn: &Conn) -> Result<Value, MethodError> {
        let raw = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.playback.clone().ok_or_else(not_configured)?;
            inner.run_active()?;
            run.config.clone()
        };
        let mut params = Map::new();
        params.insert("config".into(), raw);
        let r = self.playback_configure(conn, &params).await?;
        Ok(r["state"].clone())
    }

    pub(super) async fn playback_clear(&self, conn: &Conn) -> Result<Value, MethodError> {
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        if inner.playback_live() {
            inner.run_active()?;
        }
        inner.playback = None;
        let state = prun::state_json(None);
        inner.playback_state_key = prun::state_key(None);
        self.emit("playback.state", state.clone());
        Ok(state)
    }

    pub(super) async fn playback_state_now(&self) -> Value {
        let inner = self.inner.lock().await;
        let v = inner.playback.as_ref().map(|r| r.view());
        prun::state_json(v.as_deref())
    }

    pub(super) async fn playback_log(
        &self,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let since = since_param(params)?.unwrap_or(0);
        let inner = self.inner.lock().await;
        let (entries, next) = match &inner.playback {
            Some(r) => r.shared.log_since(since),
            None => (Vec::new(), 0),
        };
        Ok(json!({
            "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
            "next": next,
        }))
    }

    /// A run published `v`: `playback.state` when what it shows changed, and its new log
    /// entries.
    pub(super) async fn playback_changed(&self, run_id: &str, v: &PlaybackView) {
        let mut inner = self.inner.lock().await;
        let Some(run) = inner.playback.clone() else {
            return;
        };
        if run.id != run_id {
            return;
        }
        let key = prun::state_key(Some(v));
        if key != inner.playback_state_key {
            inner.playback_state_key = key;
            self.emit("playback.state", prun::state_json(Some(v)));
        }
        let since = if inner.playback_log_sent.0 == run.id {
            inner.playback_log_sent.1
        } else {
            0
        };
        let (entries, next) = run.shared.log_since(since);
        inner.playback_log_sent = (run.id.clone(), next);
        if !entries.is_empty() {
            self.emit(
                "playback.log",
                json!({
                    "run_id": run.id,
                    "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
                }),
            );
        }
    }

    /// Publish `run`'s `playback.live` ten times a second from start until it has
    /// finished, then once more, to every connection.
    fn spawn_playback_live(self: &Arc<Self>, run: Arc<PlaybackRun>) {
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
                    .playback
                    .as_ref()
                    .is_some_and(|r| Arc::ptr_eq(r, &run));
                let v = run.view();
                let last = v.state == State::Finished || !current;
                let now = Instant::now();
                if last || last_live.is_none_or(|t| now + DATA_SLACK >= t + LIVE_PERIOD) {
                    last_live = Some(now);
                    let mut arrays = Arrays::new();
                    let data = {
                        let d = match run.display.lock() {
                            Ok(d) => d,
                            Err(p) => p.into_inner(),
                        };
                        prun::live_json(&v, &d, &mut arrays)
                    };
                    let seq = svc.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    match codec::encode(&protocol::event("playback.live", seq, data), &arrays) {
                        Ok(frame) => {
                            let _ = svc.events.send(Arc::new(Out::Binary(frame)));
                        }
                        Err(e) => eprintln!("sds-control: playback.live frame not encoded: {e}"),
                    }
                }
                if last {
                    break;
                }
            }
        });
    }
}

/// Begin `run`'s controlled stop, named for `by`.
pub(super) fn stop_playback(run: &PlaybackRun, by: String) {
    run.stop(&by);
}

/// Abort `run` for `by`.
pub(super) fn abort_playback(run: &PlaybackRun, by: String) {
    run.abort(&by);
}
