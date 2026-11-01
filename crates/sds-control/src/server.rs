//! The WebSocket server: one [`Service`] shared by every connection.
//!
//! A run belongs to the service, not to a connection. One connection at a time holds
//! control ([`crate::session`]); every connection receives every event. Requests on
//! one connection are answered independently (a slow `random.abort` does not hold up
//! a `random.state`), so responses can arrive out of request order; the `id` pairs
//! them.
//!
//! The service listens on loopback only ([`check_loopback`]): every client is on the
//! computer that runs the test.

use crate::codec::{self, Arrays, Frame};
use crate::profiles;
use crate::protocol::{self, ErrorCode, MethodError, Request};
use crate::random::backend::{ABORT, STOP};
use crate::random::config as rconfig;
use crate::random::run::{self as rrun, ArmContext, Cmd, LevelTarget, RunPlan, RunShared};
use crate::random::view::{self, RunView, State, StationRef};
use crate::random::RandomRun;
use crate::session::{self, ConnId, Session};
use crate::sim::{self, RunRequest, SimRun};
use crate::station::{self, NiScanner, Selection, StationPlan, Stations};
use crate::table::{self, StationContext, Table};
use futures_util::{SinkExt, StreamExt};
use hertzforge_random::RunMode;
use serde_json::{json, Map, Value};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

/// Display event topic of the `sim.*` runs.
pub const DISPLAY_TOPIC: &str = "random.display";
/// Display event period (5 per second), for `random.display` and `random.live`.
pub const DISPLAY_PERIOD: Duration = Duration::from_millis(200);
/// How long `sim.stop` waits for the run to wind down.
const STOP_WAIT: Duration = Duration::from_secs(15);
/// Events buffered per connection before a slow client starts losing them.
const EVENT_BUFFER: usize = 256;
/// How long an arm may take: opening and committing NI tasks takes seconds.
const ARM_WAIT: Duration = Duration::from_secs(120);

/// The methods this service serves, for the unknown-method message.
const METHODS: &str = "session.state, session.claim, session.release, station.list, \
    station.select, channels.get, channels.set, random.configure, random.arm, random.start, \
    random.proceed, random.level, random.mode, random.stop, random.abort, random.reset, \
    random.clear, random.state, random.log, sim.profiles, sim.start, sim.stop and sim.state";

/// Refuse to serve anywhere but loopback. `localhost` and any loopback address pass.
pub fn check_loopback(host: &str) -> Result<(), String> {
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(());
    }
    let ip: Option<IpAddr> = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .ok();
    match ip {
        Some(ip) if ip.is_loopback() => Ok(()),
        _ => Err(format!(
            "sds-control serves loopback only, and \"{host}\" is not a loopback address. Every \
             client of the control service runs on the computer that runs the test; use \
             --host 127.0.0.1 (the default), ::1 or localhost."
        )),
    }
}

/// How the service is set up. The defaults are the protocol's; tests shorten the
/// timers, give a fixed NI scan and run the simulator unpaced.
pub struct ServiceOptions {
    pub heartbeat: Duration,
    pub timeout: Duration,
    pub scanner: Arc<dyn NiScanner>,
    /// The local data directory: the NI hardware lock lives here.
    pub data_root: PathBuf,
    /// The controller's capture root (`None`: its own local default).
    pub capture_root: Option<PathBuf>,
    /// Simulated seconds per wall second for `random.*` on the simulator.
    pub sim_speed: f64,
    /// What the hardware lock names as this service's address.
    pub bind: String,
}

impl Default for ServiceOptions {
    fn default() -> Self {
        ServiceOptions {
            heartbeat: session::HEARTBEAT,
            timeout: session::TIMEOUT,
            scanner: Arc::new(station::DriverScanner),
            data_root: hertzforge_capture::default_data_root(),
            capture_root: None,
            sim_speed: 1.0,
            bind: format!("ws://{}:{}/", crate::DEFAULT_HOST, crate::DEFAULT_PORT),
        }
    }
}

/// A frame for every connection.
#[derive(Debug, Clone)]
pub enum Out {
    Text(String),
    Binary(Vec<u8>),
}

/// A connection, as the service sees it.
#[derive(Debug, Clone)]
pub struct Conn {
    pub id: ConnId,
    pub peer: String,
}

#[derive(Default)]
struct SimSlot {
    run: Option<Arc<SimRun>>,
    started: u64,
}

struct Inner {
    session: Session,
    stations: Stations,
    table: Table,
    random: Option<Arc<RandomRun>>,
    runs: u64,
    sim: SimSlot,
    /// The last `random.state` sent, and how much of the current run's log.
    state_key: String,
    log_sent: (String, u64),
}

impl Inner {
    fn random_live(&self) -> bool {
        self.random
            .as_ref()
            .is_some_and(|r| r.view().state.is_live())
    }

    fn sim_running(&self) -> bool {
        self.sim.run.as_ref().is_some_and(|r| !r.is_finished())
    }

    fn ctx(&self) -> StationContext<'_> {
        StationContext {
            selection: self.stations.selected(),
            scan: self.stations.devices(),
        }
    }

    fn run_active(&self) -> Result<(), MethodError> {
        if let Some(r) = &self.random {
            let v = r.view();
            if v.state.is_live() {
                return Err(MethodError::named(
                    ErrorCode::Refused,
                    "run_active",
                    format!("Run {} is {}; stop it first.", r.id, v.state.name()),
                )
                .with_detail(json!({"run_id": r.id})));
            }
        }
        Ok(())
    }

    fn need_run(&self) -> Result<Arc<RandomRun>, MethodError> {
        self.random.clone().ok_or_else(|| {
            MethodError::named(
                ErrorCode::Refused,
                "not_configured",
                "Random is not configured; send random.configure first.",
            )
        })
    }
}

/// The control service.
pub struct Service {
    opts: ServiceOptions,
    inner: Mutex<Inner>,
    events: broadcast::Sender<Arc<Out>>,
    seq: AtomicU64,
    conns: AtomicU64,
    notify: mpsc::UnboundedSender<rrun::Notice>,
    notices: std::sync::Mutex<Option<mpsc::UnboundedReceiver<rrun::Notice>>>,
    background: AtomicBool,
}

fn unix_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

impl Service {
    pub fn new() -> Arc<Service> {
        Service::with_options(ServiceOptions::default())
    }

    pub fn with_options(opts: ServiceOptions) -> Arc<Service> {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let (notify, notices) = mpsc::unbounded_channel();
        Arc::new(Service {
            inner: Mutex::new(Inner {
                session: Session::new(opts.heartbeat, opts.timeout),
                stations: Stations::new(opts.scanner.clone()),
                table: Table::new(),
                random: None,
                runs: 0,
                sim: SimSlot::default(),
                state_key: String::new(),
                log_sent: (String::new(), 0),
            }),
            opts,
            events,
            seq: AtomicU64::new(0),
            conns: AtomicU64::new(0),
            notify,
            notices: std::sync::Mutex::new(Some(notices)),
            background: AtomicBool::new(false),
        })
    }

    /// Subscribe to event frames.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Out>> {
        self.events.subscribe()
    }

    /// A new connection's identity.
    pub fn connect(&self, peer: String) -> Conn {
        Conn {
            id: self.conns.fetch_add(1, Ordering::Relaxed) + 1,
            peer,
        }
    }

    fn emit(&self, topic: &str, data: Value) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let msg = protocol::event(topic, seq, data);
        let _ = self.events.send(Arc::new(Out::Text(msg.to_string())));
    }

    fn emit_binary(&self, topic: &str, data: Value, arrays: &Arrays) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        match codec::encode(&protocol::event(topic, seq, data), arrays) {
            // No subscribers is not an error: the run goes on unwatched.
            Ok(frame) => {
                let _ = self.events.send(Arc::new(Out::Binary(frame)));
            }
            Err(e) => eprintln!("sds-control: {topic} frame not encoded: {e}"),
        }
    }

    fn session_changed(&self, inner: &Inner) {
        let live = inner.random_live();
        self.emit(
            "session.changed",
            inner.session.changed(Instant::now(), live),
        );
    }

    fn station_changed(&self, inner: &Inner) {
        self.emit("station.changed", inner.stations.list());
        self.emit("channels.changed", inner.table.get(&inner.ctx()));
    }

    /// Start the background tasks (the run watcher and the orphan clock). Idempotent.
    pub fn start_background(self: &Arc<Self>) {
        if self.background.swap(true, Ordering::SeqCst) {
            return;
        }
        let notices = self.notices.lock().ok().and_then(|mut n| n.take());
        if let Some(mut notices) = notices {
            let svc = self.clone();
            tokio::spawn(async move {
                while let Some((run_id, view)) = notices.recv().await {
                    svc.run_changed(&run_id, &view).await;
                }
            });
        }
        let svc = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(100));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                svc.orphan_clock().await;
            }
        });
    }

    /// A run published `v`: send `random.state` if what it shows changed, and the
    /// run's new log entries.
    async fn run_changed(&self, run_id: &str, v: &RunView) {
        let mut inner = self.inner.lock().await;
        let Some(run) = inner.random.clone() else {
            return;
        };
        if run.id != run_id {
            return;
        }
        let key = view::state_key(Some(v));
        if key != inner.state_key {
            inner.state_key = key;
            self.emit("random.state", view::state_json(Some(v)));
        }
        let since = if inner.log_sent.0 == run.id {
            inner.log_sent.1
        } else {
            0
        };
        let (entries, next) = run.log_since(since);
        inner.log_sent = (run.id.clone(), next);
        if !entries.is_empty() {
            self.emit(
                "random.log",
                json!({"entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>()}),
            );
        }
    }

    /// Run the orphan clock: give up an absent seat, and safe-stop an unattended run.
    async fn orphan_clock(&self) {
        let mut inner = self.inner.lock().await;
        let live = inner.random_live();
        let e = inner.session.expire(Instant::now(), live);
        if e.changed {
            self.session_changed(&inner);
        }
        if e.safe_stop {
            if let Some(run) = inner.random.clone() {
                drop(inner);
                stop_run(
                    &run,
                    "safe_stop",
                    "the service (nobody held control)".into(),
                    false,
                )
                .await;
            }
        }
    }

    /// The connection went: with a goodbye (a clean close) control is free at once;
    /// without one, the seat is kept for the timeout.
    pub async fn disconnected(&self, conn: &Conn, goodbye: bool) {
        let mut inner = self.inner.lock().await;
        let now = Instant::now();
        let live = inner.random_live();
        let changed = if goodbye {
            inner.session.release(conn.id, now, live)
        } else {
            inner.session.dropped(conn.id, now)
        };
        if changed {
            self.session_changed(&inner);
        }
    }

    /// The `random.state` object now.
    async fn state_now(&self) -> Value {
        let inner = self.inner.lock().await;
        let v = inner.random.as_ref().map(|r| r.view());
        view::state_json(v.as_deref())
    }

    /// Answer one request from `conn`.
    pub async fn call(
        self: &Arc<Self>,
        conn: &Conn,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        match method {
            "sim.profiles" => Ok(json!({
                "profiles": profiles::all().iter().map(|p| p.describe()).collect::<Vec<_>>()
            })),
            "sim.start" => self.sim_start(params).await,
            "sim.stop" => self.sim_stop().await,
            "sim.state" => Ok(self.sim_state().await),

            "session.state" => {
                let inner = self.inner.lock().await;
                Ok(inner
                    .session
                    .state(conn.id, Instant::now(), inner.random_live()))
            }
            "session.claim" => self.session_claim(conn, params).await,
            "session.release" => {
                let mut inner = self.inner.lock().await;
                let now = Instant::now();
                let live = inner.random_live();
                if inner.session.release(conn.id, now, live) {
                    self.session_changed(&inner);
                }
                Ok(json!({"role": "observer", "orphan_s": inner.session.orphan_s(now, live)}))
            }

            "station.list" => self.station_list(params).await,
            "station.select" => self.station_select(conn, params).await,
            "channels.get" => {
                let inner = self.inner.lock().await;
                Ok(inner.table.get(&inner.ctx()))
            }
            "channels.set" => self.channels_set(conn, params).await,

            "random.configure" => self.random_configure(conn, params).await,
            "random.arm" => self.random_arm(conn, params, false).await,
            "random.start" => self.random_arm(conn, params, true).await,
            "random.proceed" => self.random_proceed(conn).await,
            "random.level" => self.random_level(conn, params).await,
            "random.mode" => self.random_mode(conn, params).await,
            "random.stop" => self.random_end(conn, false).await,
            "random.abort" => self.random_end(conn, true).await,
            "random.reset" => self.random_reset(conn).await,
            "random.clear" => self.random_clear(conn).await,
            "random.state" => Ok(self.state_now().await),
            "random.log" => self.random_log(params).await,
            other => Err(MethodError::new(
                ErrorCode::UnknownMethod,
                format!("the control service has no method \"{other}\"; it serves {METHODS}"),
            )),
        }
    }

    // ---- session ------------------------------------------------------------------

    async fn session_claim(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let name = protocol::named_str(params, "name")?;
        if name.is_some_and(str::is_empty) {
            return Err(MethodError::bad_params(
                "name",
                "\"name\" must not be empty",
            ));
        }
        let lease = protocol::named_str(params, "lease")?;
        let mut inner = self.inner.lock().await;
        let c = inner
            .session
            .claim(conn.id, name, lease, &conn.peer, Instant::now())?;
        self.session_changed(&inner);
        Ok(json!({
            "role": "controller", "lease": c.lease, "resumed": c.resumed,
            "heartbeat_s": inner.session.heartbeat().as_secs_f64(),
            "timeout_s": inner.session.timeout().as_secs_f64(),
        }))
    }

    // ---- stations and the table ---------------------------------------------------

    async fn scan(&self) -> station::Scan {
        let scanner = self.opts.scanner.clone();
        tokio::task::spawn_blocking(move || station::take_scan(&*scanner))
            .await
            .unwrap_or_else(|e| station::Scan {
                at_unix: unix_s(),
                result: Err(station::ScanFailure {
                    reason: "scan_failed",
                    message: format!("The scan did not complete: {e}"),
                }),
            })
    }

    async fn station_list(&self, params: &Map<String, Value>) -> Result<Value, MethodError> {
        let rescan = protocol::param_bool(params, "rescan")?.unwrap_or(false);
        let never = self.inner.lock().await.stations.last_scan().is_none();
        if rescan || never {
            let scan = self.scan().await;
            let mut inner = self.inner.lock().await;
            inner.stations.record_scan(scan);
            self.station_changed(&inner);
            return Ok(inner.stations.list());
        }
        Ok(self.inner.lock().await.stations.list())
    }

    async fn station_select(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            inner.run_active()?;
        }
        match protocol::named_str(params, "kind")? {
            Some("sim") => {
                let mut inner = self.inner.lock().await;
                inner.session.require_controller(conn.id)?;
                inner.run_active()?;
                inner.stations.select_sim();
                self.station_changed(&inner);
                Ok(json!({"selected": inner.stations.selected_json()}))
            }
            Some("ni") => {
                let devices: Vec<String> = match params.get("devices") {
                    Some(Value::Array(a))
                        if !a.is_empty()
                            && a.iter().all(|d| d.as_str().is_some_and(|s| !s.is_empty())) =>
                    {
                        a.iter()
                            .filter_map(|d| d.as_str().map(str::to_owned))
                            .collect()
                    }
                    _ => {
                        return Err(MethodError::bad_params(
                            "devices",
                            "Selecting NI hardware names its devices: \
                             {\"kind\":\"ni\",\"devices\":[...]}.",
                        ))
                    }
                };
                let scan = self.scan().await;
                let busy = station::hardware_holder(&self.opts.data_root);
                let mut inner = self.inner.lock().await;
                inner.session.require_controller(conn.id)?;
                inner.run_active()?;
                let r = inner.stations.select_ni(devices, scan, busy);
                // The scan changed whether or not the selection was accepted.
                self.station_changed(&inner);
                r?;
                Ok(json!({"selected": inner.stations.selected_json()}))
            }
            other => Err(MethodError::bad_params(
                "kind",
                format!(
                    "\"kind\" is \"sim\" or \"ni\", not {}",
                    other.map_or("absent".to_owned(), |k| format!("\"{k}\""))
                ),
            )),
        }
    }

    async fn channels_set(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let revision = params
            .get("revision")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                MethodError::bad_params(
                    "revision",
                    "channels.set quotes the \"revision\" it edited, a whole number",
                )
            })?;
        let channels = params.get("channels").ok_or_else(|| {
            MethodError::bad_params("channels", "channels.set takes {\"revision\",\"channels\"}")
        })?;
        let table = table::parse_rows(channels)?;
        let dry_run = protocol::param_bool(params, "dry_run")?.unwrap_or(false);
        let mut inner = self.inner.lock().await;
        let report = table::validate(&table, &inner.ctx());
        if dry_run {
            return Ok(json!({"revision": inner.table.revision, "report": report.to_json()}));
        }
        inner.session.require_controller(conn.id)?;
        inner.run_active()?;
        if revision != inner.table.revision {
            let current = inner.table.revision;
            return Err(MethodError::named(
                ErrorCode::Refused,
                "stale_revision",
                format!(
                    "The table changed since revision {revision} (it is now {current}); read it \
                     again and review the change before sending."
                ),
            )
            .with_detail(json!({"current": current, "provided": revision})));
        }
        if !report.errors.is_empty() {
            return Err(table_invalid(&report, "so it was not written"));
        }
        inner.table.table = table;
        inner.table.revision += 1;
        self.emit("channels.changed", inner.table.get(&inner.ctx()));
        Ok(json!({"revision": inner.table.revision, "report": report.to_json()}))
    }

    // ---- Random -------------------------------------------------------------------

    async fn random_configure(
        &self,
        conn: &Conn,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let config = params
            .get("config")
            .filter(|c| c.is_object())
            .ok_or_else(|| {
                MethodError::bad_params("config", "random.configure takes {\"config\":{...}}")
            })?;
        let cfg = rconfig::parse(config)?;
        let dry_run = protocol::param_bool(params, "dry_run")?.unwrap_or(false);
        let mut inner = self.inner.lock().await;
        let table_report = table::validate(&inner.table.table, &inner.ctx());
        let (report, derived) = cfg.check(&inner.table.table);
        if dry_run {
            let mut all = table_report.clone();
            all.extend(report.clone());
            let derived = if all.errors.is_empty() { derived } else { None };
            return Ok(json!({
                "errors": all.errors.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
                "warnings": all.warnings.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
                "derived": derived.map_or(Value::Null, |d| d.to_json()),
            }));
        }
        inner.session.require_controller(conn.id)?;
        inner.run_active()?;
        if !table_report.errors.is_empty() {
            return Err(table_invalid(&table_report, "on this station"));
        }
        let Some(derived) = derived else {
            return Err(rrun::config_invalid(&report));
        };
        let run = self.new_run(&mut inner, cfg, &derived)?;
        let v = run.view();
        Ok(json!({
            "state": view::state_json(Some(&v)),
            "derived": derived.to_json(),
            "warnings": report.warnings.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
        }))
    }

    /// Create a run (replacing the one before) and start its thread.
    fn new_run(
        &self,
        inner: &mut Inner,
        cfg: rconfig::WireConfig,
        derived: &rconfig::Derived,
    ) -> Result<Arc<RandomRun>, MethodError> {
        inner.runs += 1;
        let run_id = format!("r-{:04}", inner.runs);
        let class = inner.stations.class();
        let selected = inner.stations.selected().clone();
        let devices: Vec<(String, String)> = match &selected {
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
        };
        let station_ref = StationRef {
            kind: if selected == Selection::Sim {
                "sim"
            } else {
                "ni"
            },
            class: station::class_name(class),
            devices,
        };
        let initial = RunView {
            run_id: run_id.clone(),
            title: cfg.title.clone(),
            state: State::Configured,
            phase: "idle",
            mode: cfg.mode,
            levels_db: cfg.levels.iter().map(|l| l.0).collect(),
            full_level_s: cfg.full_level_s,
            station: station_ref.clone(),
            grid: derived.grid.clone(),
            clock: None,
            level_db: None,
            live_level_db: None,
            elapsed_s: None,
            at_level_s: None,
            remaining_at_level_s: None,
            alarm: false,
            feasibility: None,
            end: None,
            loops: 0,
            control_rms_g: None,
            reference_rms_g: None,
            drive_rms_v: None,
            lines_alarm: 0,
            lines_abort: 0,
            channels: rrun::live_channels(&inner.table.table),
            alarms: Vec::new(),
            lines: None,
            control_psd: None,
            drive_psd: None,
            streaming: false,
        };
        let shared = Arc::new(RunShared {
            view: std::sync::Mutex::new(Arc::new(initial.clone())),
            log: std::sync::Mutex::new(Vec::new()),
            signal: Arc::new(AtomicU8::new(0)),
            ended_by: std::sync::Mutex::new(None),
            notify: self.notify.clone(),
        });
        let raw = cfg.raw.clone();
        let title = cfg.title.clone();
        let plan = RunPlan {
            run_id: run_id.clone(),
            config: cfg,
            sim_speed: self.opts.sim_speed,
            capture_root: self.opts.capture_root.clone(),
            timeout_s: self.opts.timeout.as_secs_f64(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        rrun::spawn(plan, initial.clone(), shared.clone(), rx)
            .map_err(|e| MethodError::internal(format!("could not start the run thread: {e}")))?;
        let run = Arc::new(RandomRun::new(run_id, shared.clone(), tx, raw));
        inner.random = Some(run.clone());
        // The log starts at configure.
        shared.add_log(
            "state",
            format!("configured \"{title}\" on {}", initial.station.class),
        );
        let _ = self.notify.send((run.id.clone(), Arc::new(initial)));
        Ok(run)
    }

    /// What a run arms on now: the current table and the selected station.
    fn arm_context(&self, inner: &Inner, run: &RandomRun) -> Result<ArmContext, MethodError> {
        let report = table::validate(&inner.table.table, &inner.ctx());
        if !report.errors.is_empty() {
            return Err(table_invalid(
                &report,
                "on this station, so the run cannot arm",
            ));
        }
        let fs_hz = run.view().grid.fs_hz;
        let plan = match inner.stations.selected() {
            Selection::Sim => StationPlan::Sim { fs_hz },
            Selection::Ni { devices } => {
                let (inputs, outputs) = hertzforge_station::physical_from_table(&inner.table.table);
                StationPlan::Ni {
                    devices: devices.clone(),
                    inputs,
                    outputs,
                    fs_hz,
                }
            }
        };
        Ok(ArmContext {
            table: inner.table.table.clone(),
            station: plan,
            scanner: self.opts.scanner.clone(),
            data_root: self.opts.data_root.clone(),
            bind: self.opts.bind.clone(),
        })
    }

    async fn random_arm(
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
        let (run, ctx) = {
            let inner = self.inner.lock().await;
            inner.session.require_controller(conn.id)?;
            let run = inner.need_run()?;
            let v = run.view();
            if v.state == State::Finished {
                return Err(MethodError::named(
                    ErrorCode::Refused,
                    "run_finished",
                    "This run has finished; reset or configure again to run.",
                )
                .with_detail(json!({"outcome": v.end.as_ref().map(|e| e.outcome)})));
            }
            if v.state.is_busy() || (!start && v.state == State::Armed) {
                inner.run_active()?;
            }
            if inner.sim_running() {
                let id = inner.sim.run.as_ref().map(|r| r.run_id().to_owned());
                return Err(MethodError::named(
                    ErrorCode::Refused,
                    "run_active",
                    format!(
                        "Simulation {} is running; stop it with sim.stop first.",
                        id.as_deref().unwrap_or("")
                    ),
                )
                .with_detail(json!({ "run_id": id })));
            }
            let ctx = self.arm_context(&inner, &run)?;
            (run, ctx)
        };
        if start {
            run.ask(|reply| Cmd::Start { ctx, reply }, ARM_WAIT).await?;
            self.spawn_live(run.clone());
        } else {
            run.ask(|reply| Cmd::Arm { ack, ctx, reply }, ARM_WAIT)
                .await?;
        }
        Ok(view::state_json(Some(&run.view())))
    }

    /// Publish `random.live` for `run` five times a second until it has finished,
    /// then once more with the final state.
    fn spawn_live(self: &Arc<Self>, run: Arc<RandomRun>) {
        let svc = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(DISPLAY_PERIOD);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let current = svc
                    .inner
                    .lock()
                    .await
                    .random
                    .as_ref()
                    .is_some_and(|r| r.id == run.id);
                let v = run.view();
                if !v.streaming {
                    if !current {
                        break;
                    }
                    continue;
                }
                let mut arrays = Arrays::new();
                let data = view::live_json(&v, &mut arrays);
                svc.emit_binary("random.live", data, &arrays);
                if v.state == State::Finished || !current {
                    break;
                }
            }
        });
    }

    async fn controlled_run(&self, conn: &Conn) -> Result<Arc<RandomRun>, MethodError> {
        let inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        inner.need_run()
    }

    async fn random_proceed(&self, conn: &Conn) -> Result<Value, MethodError> {
        let run = self.controlled_run(conn).await?;
        let v = run.view();
        if v.state != State::Review {
            return Err(wrong_state(&v, "random.proceed"));
        }
        run.ask(|reply| Cmd::Proceed { reply }, wait_for(&run))
            .await?;
        Ok(view::state_json(Some(&run.view())))
    }

    async fn random_level(
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
                    "random.level takes one of \"delta_db\" and \"to_db\".",
                ))
            }
        };
        let run = self.controlled_run(conn).await?;
        let v = run.view();
        if v.mode != RunMode::Manual {
            return Err(MethodError::named(
                ErrorCode::Refused,
                "manual_mode_required",
                "Level changes need the manual ladder; switch with random.mode first.",
            )
            .with_detail(json!({"mode": view::mode_name(v.mode)})));
        }
        if !(v.state == State::Running && matches!(v.phase, "engaged" | "full_level")) {
            return Err(wrong_state(&v, "random.level"));
        }
        let level = run
            .ask(|reply| Cmd::Level { target, reply }, wait_for(&run))
            .await?;
        Ok(json!({"level_db": protocol::num(rconfig::round(level, 2))}))
    }

    async fn random_mode(
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
                    "\"mode\" is \"auto\" or \"manual\".",
                ))
            }
        };
        let run = self.controlled_run(conn).await?;
        let v = run.view();
        if v.state == State::Finished {
            return Err(wrong_state(&v, "random.mode"));
        }
        if v.mode != mode {
            run.ask(|reply| Cmd::Mode { mode, reply }, wait_for(&run))
                .await?;
        }
        Ok(json!({"mode": view::mode_name(mode)}))
    }

    /// `random.stop` and `random.abort`: open to every connection.
    async fn random_end(&self, conn: &Conn, abort: bool) -> Result<Value, MethodError> {
        let (run, by) = {
            let inner = self.inner.lock().await;
            let Some(run) = inner.random.clone() else {
                return Ok(view::state_json(None));
            };
            (run, inner.session.client_name(conn.id, &conn.peer))
        };
        if !run.view().state.is_live() {
            return Ok(view::state_json(Some(&run.view())));
        }
        run.ended_by(&by);
        if abort {
            run.signal(ABORT);
            let wait = wait_for(&run);
            let _ = run.ask(|reply| Cmd::Abort { by, reply }, wait).await;
        } else {
            stop_run(&run, "operator_stop", by, true).await;
        }
        Ok(view::state_json(Some(&run.view())))
    }

    async fn random_reset(&self, conn: &Conn) -> Result<Value, MethodError> {
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        let run = inner.need_run()?;
        if run.view().state.is_busy() {
            inner.run_active()?;
        }
        let cfg = rconfig::parse(&run.config)?;
        let table_report = table::validate(&inner.table.table, &inner.ctx());
        if !table_report.errors.is_empty() {
            return Err(table_invalid(&table_report, "on this station"));
        }
        let (report, derived) = cfg.check(&inner.table.table);
        let Some(derived) = derived else {
            return Err(rrun::config_invalid(&report));
        };
        let run = self.new_run(&mut inner, cfg, &derived)?;
        let v = run.view();
        Ok(view::state_json(Some(&v)))
    }

    async fn random_clear(&self, conn: &Conn) -> Result<Value, MethodError> {
        let mut inner = self.inner.lock().await;
        inner.session.require_controller(conn.id)?;
        if let Some(run) = &inner.random {
            if run.view().state.is_busy() {
                inner.run_active()?;
            }
        }
        inner.random = None;
        let state = view::state_json(None);
        inner.state_key = view::state_key(None);
        self.emit("random.state", state.clone());
        Ok(state)
    }

    async fn random_log(&self, params: &Map<String, Value>) -> Result<Value, MethodError> {
        let since = match params.get("since") {
            None | Some(Value::Null) => 0,
            Some(v) => v.as_u64().ok_or_else(|| {
                MethodError::bad_params("since", "\"since\" is a whole number from 0")
            })?,
        };
        let inner = self.inner.lock().await;
        let (entries, next) = match &inner.random {
            Some(r) => r.log_since(since),
            None => (Vec::new(), 0),
        };
        Ok(json!({
            "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
            "next": next,
        }))
    }

    // ---- sim.* (superseded for running tests; the Simulate workspace's demos) ------

    async fn sim_start(
        self: &Arc<Self>,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let profile_id = protocol::param_str(params, "profile")?.ok_or_else(|| {
            MethodError::bad_request(format!(
                "sim.start needs \"profile\"; sim.profiles lists them ({})",
                profiles::ids().join(", ")
            ))
        })?;
        let profile = profiles::find(profile_id).ok_or_else(|| {
            MethodError::not_found(format!(
                "no profile named \"{profile_id}\"; sim.profiles lists the ones that exist ({})",
                profiles::ids().join(", ")
            ))
        })?;
        let request = RunRequest {
            profile,
            level_db: protocol::param_f64(params, "level_db")?.unwrap_or(0.0),
            speed: protocol::param_f64(params, "speed")?.unwrap_or(1.0),
            duration_s: protocol::param_f64(params, "duration_s")?
                .unwrap_or(sim::DEFAULT_DURATION_S),
        };
        request.validate().map_err(MethodError::bad_request)?;

        let mut inner = self.inner.lock().await;
        if let Some(r) = &inner.random {
            if r.view().state.is_live() {
                return Err(MethodError::named(
                    ErrorCode::Refused,
                    "run_active",
                    format!(
                        "Run {} holds the station; stop it before starting a simulation.",
                        r.id
                    ),
                )
                .with_detail(json!({"run_id": r.id})));
            }
        }
        if let Some(run) = &inner.sim.run {
            if !run.is_finished() {
                return Err(MethodError::named(
                    ErrorCode::Refused,
                    "run_active",
                    format!("run {} is still running; send sim.stop first", run.run_id()),
                )
                .with_detail(json!({"run_id": run.run_id()})));
            }
        }
        inner.sim.started += 1;
        let run_id = format!("sim-{}-{}", unix_ms(), inner.sim.started);
        let echo = json!({
            "profile": request.profile.id,
            "level_db": request.level_db,
            "speed": request.speed,
            "duration_s": request.duration_s,
        });
        let run = tokio::task::spawn_blocking(move || SimRun::start(run_id, request))
            .await
            .map_err(|e| MethodError::internal(format!("the run start task failed: {e}")))?
            .map_err(MethodError::refused)?;
        let run = Arc::new(run);
        inner.sim.run = Some(run.clone());
        drop(inner);
        self.spawn_display(run.clone());

        let mut result = json!({"run_id": run.run_id(), "source": "SIM"});
        if let (Value::Object(r), Value::Object(e)) = (&mut result, echo) {
            r.extend(e);
        }
        Ok(result)
    }

    async fn sim_stop(&self) -> Result<Value, MethodError> {
        let run = self.inner.lock().await.sim.run.clone();
        let Some(run) = run else {
            return Ok(json!({"state": "idle", "run_id": Value::Null}));
        };
        run.request_stop();
        let waiter = run.clone();
        let finished = tokio::task::spawn_blocking(move || waiter.wait(STOP_WAIT))
            .await
            .unwrap_or(false);
        if !finished {
            return Err(MethodError::internal(format!(
                "run {} did not stop within {} s; the stop request stands",
                run.run_id(),
                STOP_WAIT.as_secs()
            )));
        }
        Ok(json!({"state": run.protocol_state(), "run_id": run.run_id()}))
    }

    async fn sim_state(&self) -> Value {
        let run = self.inner.lock().await.sim.run.clone();
        match run {
            None => json!({"state": "idle", "run_id": Value::Null}),
            Some(run) => {
                let snap = run.snapshot();
                let finish = snap.finish.as_ref();
                json!({
                    "state": run.protocol_state(),
                    "run_id": run.run_id(),
                    "source": "SIM",
                    "profile": run.request().profile.id,
                    "phase": sim::phase_name(snap.phase),
                    "elapsed_s": crate::views::round_to(run.elapsed_s(), 3),
                    "level_db": crate::views::round_to(snap.level_db, 2),
                    "outcome": finish.map(|f| f.outcome.as_str()),
                    "reason": finish.map(|f| f.reason.clone()),
                })
            }
        }
    }

    /// Publish `random.display` for a `sim.*` run every [`DISPLAY_PERIOD`] until it
    /// has finished, then once more with the final state and the reason it ended.
    fn spawn_display(self: &Arc<Self>, run: Arc<SimRun>) {
        let svc = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(DISPLAY_PERIOD);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let finished = run.is_finished();
                let mut arrays = Arrays::new();
                let data = run.display_data(&mut arrays);
                svc.emit_binary(DISPLAY_TOPIC, data, &arrays);
                if finished {
                    break;
                }
            }
        });
    }
}

/// How long to wait for a run to answer a request: three of its control loops and
/// a margin.
fn wait_for(run: &RandomRun) -> Duration {
    let loop_s = run.view().grid.loop_s.max(0.05);
    Duration::from_secs_f64(loop_s * 3.0 + 10.0)
}

/// The controlled stop (`random.stop` and the safe stop). During system
/// identification and the gate the station stops at the next block.
async fn stop_run(run: &RandomRun, outcome: &'static str, by: String, wait: bool) {
    let v = run.view();
    if !v.state.is_live() {
        return;
    }
    if v.state == State::Running && matches!(v.phase, "sysid" | "coherence_gate") {
        run.signal(STOP);
    }
    if wait {
        let w = wait_for(run);
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
    } else {
        let _ = run.send(Cmd::Stop {
            outcome,
            by,
            reply: None,
        });
    }
}

fn wrong_state(v: &RunView, verb: &str) -> MethodError {
    MethodError::named(
        ErrorCode::Refused,
        "wrong_state",
        format!(
            "{verb} does not apply while the run is {} ({}).",
            v.state.name(),
            v.phase.replace('_', " ")
        ),
    )
    .with_detail(json!({"state": v.state.name(), "phase": v.phase}))
}

fn table_invalid(report: &table::Report, what: &str) -> MethodError {
    let n = report.errors.len();
    MethodError::named(
        ErrorCode::BadRequest,
        "table_invalid",
        format!(
            "The channel table has {n} error{}, {what}.",
            if n == 1 { "" } else { "s" }
        ),
    )
    .with_detail(report.to_json())
}

/// Accept connections until the listener fails.
pub async fn serve(listener: TcpListener, service: Arc<Service>) -> std::io::Result<()> {
    service.start_background();
    loop {
        let (stream, peer) = listener.accept().await?;
        let service = service.clone();
        tokio::spawn(async move {
            if let Err(e) = connection(stream, peer.to_string(), service).await {
                eprintln!("sds-control: connection ended: {e}");
            }
        });
    }
}

async fn connection(
    stream: TcpStream,
    peer: String,
    service: Arc<Service>,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut sink, mut source) = ws.split();
    sink.send(Message::Text(protocol::hello().to_string()))
        .await?;
    let conn = service.connect(peer);

    // The writer owns the sink: responses and pings arrive on `out`, events on
    // `events`.
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(64);
    let mut events = service.subscribe();
    let writer = tokio::spawn(async move {
        loop {
            let msg = tokio::select! {
                m = out_rx.recv() => match m {
                    Some(m) => m,
                    None => break,
                },
                e = events.recv() => match e {
                    Ok(frame) => match frame.as_ref() {
                        Out::Text(t) => Message::Text(t.clone()),
                        Out::Binary(b) => Message::Binary(b.clone()),
                    },
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            };
            if sink.send(msg).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // The heartbeat: a ping every `heartbeat`; three with nothing inbound and the
    // connection is treated as gone.
    let heartbeat = service.opts.heartbeat;
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_inbound = Instant::now();
    let mut goodbye = false;
    loop {
        let msg = tokio::select! {
            m = source.next() => m,
            _ = ping.tick() => {
                if last_inbound.elapsed() > heartbeat * session::MISSED_HEARTBEATS {
                    break;
                }
                if out_tx.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
                continue;
            }
        };
        let Some(Ok(msg)) = msg else { break };
        last_inbound = Instant::now();
        let message = match msg {
            Message::Text(text) => codec::decode_text(&text),
            Message::Binary(bytes) => codec::decode(&bytes).map(|d| d.message),
            Message::Close(_) => {
                goodbye = true;
                break;
            }
            _ => continue,
        };
        let service = service.clone();
        let out = out_tx.clone();
        let conn = conn.clone();
        tokio::spawn(async move {
            let reply = match message {
                Err(e) => protocol::error_response(
                    protocol::UNREADABLE_ID,
                    &MethodError::bad_request(format!("the frame could not be decoded: {e}")),
                ),
                Ok(message) => match protocol::parse_request(&message) {
                    Err((id, e)) => protocol::error_response(id, &e),
                    Ok(Request { id, method, params }) => {
                        match service.call(&conn, &method, &params).await {
                            Ok(result) => protocol::response(id, result),
                            Err(e) => protocol::error_response(id, &e),
                        }
                    }
                },
            };
            let frame = match codec::encode_frame(&reply, &Arrays::new()) {
                Ok(Frame::Text(t)) => Message::Text(t),
                Ok(Frame::Binary(b)) => Message::Binary(b),
                Err(e) => Message::Text(
                    protocol::error_response(
                        protocol::UNREADABLE_ID,
                        &MethodError::internal(format!("the reply could not be encoded: {e}")),
                    )
                    .to_string(),
                ),
            };
            let _ = out.send(frame).await;
        });
    }
    service.disconnected(&conn, goodbye).await;
    drop(out_tx);
    let _ = writer.await;
    Ok(())
}
