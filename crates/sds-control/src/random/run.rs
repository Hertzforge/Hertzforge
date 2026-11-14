//! One configured Random run, on its own thread.
//!
//! The thread owns the controller's `RandomService` outright and drives it the way
//! the controller's daemon does: feasibility, arm, system identification, the
//! coherence gate, the review hold, then `loop_tick` until the run ends. Nothing
//! here is control math. The socket never reaches the loop: requests arrive as
//! [`Cmd`]s the thread takes between control loops, and the thread publishes a
//! [`RunView`] after every step for the service to show.
//!
//! A stop or an abort must not wait for a long call (system identification, one
//! control loop) to finish, so the operator's signal also reaches the station
//! directly through [`Interruptible`]; the station takes the drive down with its
//! own stop. The controller's protections (the envelope, kinematic, drive and
//! divergence aborts, the station's watchdog) stay the controller's.
//!
//! The station is held only while the run is live. When it ends, however it ends,
//! the service is dropped, which stops the station (`PlantLoop`'s drop) and releases
//! the hardware lock.

use super::backend::{Interruptible, ABORT, STOP};
use super::config::{check_grid, Grid, WireConfig};
use super::record::{self as rec, Spectra};
use super::view::{
    Alarm, End, Inputs, LimitLines, LiveChannel, RecordRef, RunView, State, StationRef,
};
use crate::protocol::{num, ErrorCode, MethodError};
use crate::record::tap::{self, Recording};
use crate::record::{ProblemFn, RecordStatus, RunRecord};
use crate::station::{self, class_name, NiScanner, StationPlan};
use crate::table::{role_name, unit_name, Report};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_random::service::engine::BlockObserver;
use hertzforge_random::service::ramp::DriveRamp;
use hertzforge_random::service::report::EventKind;
use hertzforge_random::{RandomService, RunMode, RunState, ServiceError, TripKind};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// Who stops a run when the service shuts down (`Service::shutdown`). Its safe stop
/// says so; a safe stop by anyone else is the orphan clock's.
pub const SHUTDOWN_BY: &str = "the service (shutting down)";

/// A run log entry (schema/PROTOCOL.md, "Run Log").
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub seq: u64,
    pub at_unix: f64,
    pub kind: &'static str,
    pub text: String,
}

impl LogEntry {
    pub fn to_json(&self) -> Value {
        json!({"seq": self.seq, "at_unix": num(super::config::round(self.at_unix, 3)),
               "kind": self.kind, "text": self.text})
    }
}

/// A publish, as the service hears it: the run id and the view published.
pub type Notice = (String, Arc<RunView>);

/// What the run thread and the service share.
pub struct RunShared {
    pub view: Mutex<Arc<RunView>>,
    pub log: Mutex<Vec<LogEntry>>,
    /// The operator's signal to the station ([`super::backend`]).
    pub signal: Arc<AtomicU8>,
    /// Who sent the stop or abort, set with the signal so a run that ends inside a
    /// long call can name them.
    pub ended_by: Mutex<Option<String>>,
    /// A controlled stop started from the socket's thread (`RandomRun::begin_stop`):
    /// its outcome and who asked, for a ramp-down that arrives before the run
    /// thread takes the queued request.
    pub pending_stop: Mutex<Option<(&'static str, String)>>,
    /// The run's drive level ramp while it is armed (the controller's), so the
    /// socket's thread can start the stop and read the level the drive is at.
    pub ramp: Mutex<Option<Arc<DriveRamp>>>,
    /// The level the drive was at when the run was first asked to stop or abort.
    pub requested_level: Mutex<Option<f64>>,
    /// Told after every publish, with the run id and the view published, so every
    /// change of state reaches the service in order (none is folded into a later one).
    pub notify: tokio::sync::mpsc::UnboundedSender<Notice>,
    /// The run's record from arm on: every log entry is appended to it as it is made.
    pub record: Mutex<Option<crate::record::LogHandle>>,
    /// The input frames the station has delivered since it started
    /// ([`NO_FRAME`] before the first), for the record's entries.
    pub station_frame: Arc<AtomicU64>,
}

/// [`RunShared::station_frame`] before the station has delivered a frame.
pub const NO_FRAME: u64 = u64::MAX;

impl RunShared {
    /// What the service and the run thread share at configure.
    pub fn new(initial: RunView, notify: tokio::sync::mpsc::UnboundedSender<Notice>) -> RunShared {
        RunShared {
            view: Mutex::new(Arc::new(initial)),
            log: Mutex::new(Vec::new()),
            signal: Arc::new(AtomicU8::new(0)),
            ended_by: Mutex::new(None),
            pending_stop: Mutex::new(None),
            ramp: Mutex::new(None),
            requested_level: Mutex::new(None),
            notify,
            record: Mutex::new(None),
            station_frame: Arc::new(AtomicU64::new(NO_FRAME)),
        }
    }
}

impl RunShared {
    pub fn view(&self) -> Arc<RunView> {
        match self.view.lock() {
            Ok(v) => v.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    pub fn log_since(&self, since: u64) -> (Vec<LogEntry>, u64) {
        let log = match self.log.lock() {
            Ok(l) => l,
            Err(p) => p.into_inner(),
        };
        let next = log.len() as u64;
        let from = (since as usize).min(log.len());
        (log[from..].to_vec(), next)
    }

    pub fn add_log(&self, kind: &'static str, text: impl Into<String>) {
        self.add_log_data(kind, text, None);
    }

    /// A log entry with structured facts for the record (`data`): the level of a
    /// level change, the facts of an alarm, a trip or an overload, who acted.
    pub fn add_log_data(&self, kind: &'static str, text: impl Into<String>, data: Option<Value>) {
        let mut log = match self.log.lock() {
            Ok(l) => l,
            Err(p) => p.into_inner(),
        };
        let seq = log.len() as u64;
        let entry = LogEntry {
            seq,
            at_unix: now_unix(),
            kind,
            text: text.into(),
        };
        // Appended in the order the entries are made: the hand-off never waits.
        if let Ok(record) = self.record.lock() {
            if let Some(r) = record.as_ref() {
                r.append(self.record_entry(&entry, data));
            }
        }
        log.push(entry);
    }

    /// An entry as the record holds it: the wire's members, the station frame
    /// when the station has delivered one, and `data` when there is any.
    pub fn record_entry(&self, e: &LogEntry, data: Option<Value>) -> Value {
        let frame = self.station_frame.load(Ordering::Relaxed);
        let mut v = json!({"seq": e.seq, "at_unix": e.at_unix,
                           "frame": if frame == NO_FRAME { Value::Null } else { Value::from(frame) },
                           "kind": e.kind, "text": e.text});
        if let (Some(d), Value::Object(m)) = (data, &mut v) {
            m.insert("data".into(), d);
        }
        v
    }

    /// Start appending the log to the run's record: the entries made so far first,
    /// in order, then every later entry as it is made. Done under the log's lock, so
    /// none is missed or written twice.
    pub fn attach_record(&self, handle: crate::record::LogHandle) {
        let log = match self.log.lock() {
            Ok(l) => l,
            Err(p) => p.into_inner(),
        };
        for e in log.iter() {
            handle.append(self.record_entry(e, None));
        }
        if let Ok(mut r) = self.record.lock() {
            *r = Some(handle);
        }
    }

    /// Stop appending to the record (the run has ended).
    pub fn detach_record(&self) {
        let _log = match self.log.lock() {
            Ok(l) => l,
            Err(p) => p.into_inner(),
        };
        if let Ok(mut r) = self.record.lock() {
            *r = None;
        }
    }

    fn publish(&self, view: RunView) {
        let id = view.run_id.clone();
        let view = Arc::new(view);
        match self.view.lock() {
            Ok(mut v) => *v = view.clone(),
            Err(p) => *p.into_inner() = view.clone(),
        }
        let _ = self.notify.send((id, view));
    }
}

/// A level request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LevelTarget {
    Delta(f64),
    To(f64),
}

/// What a run arms on, taken from the service when the request arrives.
pub struct ArmContext {
    pub table: ChannelTable,
    pub station: StationPlan,
    pub scanner: Arc<dyn NiScanner>,
    /// The service's local data directory: the hardware lock lives here.
    pub data_root: PathBuf,
    pub bind: String,
    /// The table's revision, for the run's record.
    pub revision: u64,
    /// The devices of the last scan, for the record's models and serial numbers.
    pub devices: Vec<hertzforge_hal::DiscoveredDevice>,
}

type Reply<T> = oneshot::Sender<Result<T, MethodError>>;

/// A request to the run thread.
pub enum Cmd {
    Arm {
        ack: Option<String>,
        ctx: ArmContext,
        reply: Reply<()>,
    },
    Start {
        ctx: ArmContext,
        reply: Reply<()>,
    },
    Proceed {
        reply: Reply<()>,
    },
    Level {
        target: LevelTarget,
        reply: Reply<f64>,
    },
    Mode {
        mode: RunMode,
        reply: Reply<()>,
    },
    Stop {
        outcome: &'static str,
        by: String,
        reply: Option<Reply<()>>,
    },
    Abort {
        by: String,
        reply: Reply<()>,
    },
}

/// The fixed facts of one run.
pub struct RunPlan {
    pub run_id: String,
    pub config: WireConfig,
    /// Simulated seconds per wall second on the simulator; 0 runs unpaced (tests).
    pub sim_speed: f64,
    /// The capture root handed to the controller (`None`: its local default).
    pub capture_root: Option<PathBuf>,
    /// The orphan timeout, for the safe stop's message.
    pub timeout_s: f64,
    /// How the run's record is written.
    pub record: crate::record::RecordOptions,
}

/// The run thread's state.
struct Actor {
    plan: RunPlan,
    shared: Arc<RunShared>,
    svc: Option<RandomService>,
    lock: Option<hertzforge_station::lock::LockGuard>,
    view: RunView,
    /// The table the run armed with.
    table: ChannelTable,
    started: Option<Instant>,
    ended_at: Option<f64>,
    stop_outcome: &'static str,
    stopped_by: String,
    /// Controller log events already forwarded.
    forwarded: usize,
    alarm_was: bool,
    sim_frames: Arc<AtomicU64>,
    /// The run's record, from arm until the run ends.
    record: Option<RunRecord>,
    record_status: Option<Arc<RecordStatus>>,
    /// The record's stream of the drive, when it has one.
    drive_stream: Option<usize>,
    run_uid: String,
    utc_offset_s: Option<i64>,
    /// The last control loop's spectra, kept at a level change and at the end.
    last_spectra: Option<Spectra>,
    spectra_kept: u64,
}

fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn refuse(reason: &'static str, message: impl Into<String>) -> MethodError {
    MethodError::named(ErrorCode::Refused, reason, message)
}

fn wrong_state(v: &RunView, verb: &str) -> MethodError {
    refuse(
        "wrong_state",
        format!(
            "{verb} does not apply while the run is {} ({}).",
            v.state.name(),
            v.phase.replace('_', " ")
        ),
    )
    .with_detail(json!({"state": v.state.name(), "phase": v.phase}))
}

fn arm_refused(words: impl std::fmt::Display) -> MethodError {
    refuse(
        "arm_refused",
        format!("The station refused to arm: {words}"),
    )
}

/// The pacer for the simulator: counts plant frames and, when paced, sleeps whenever
/// simulated time runs ahead of wall time. It paces the same with a stop or abort
/// pending: the station signal, not the pacer, is what ends a call early.
fn pacer(frames: Arc<AtomicU64>, fs_hz: f64, speed: f64) -> BlockObserver {
    let t0 = Instant::now();
    Arc::new(move |_input: &[f32], n: usize| {
        let total = frames.fetch_add(n as u64, Ordering::Relaxed) + n as u64;
        if speed <= 0.0 {
            return;
        }
        let ahead = total as f64 / fs_hz / speed - t0.elapsed().as_secs_f64();
        if ahead > 0.005 {
            std::thread::sleep(Duration::from_secs_f64(ahead.min(0.25)));
        }
    })
}

/// Start the run thread. Returns once the thread exists; the run is `configured`.
pub fn spawn(
    plan: RunPlan,
    initial: RunView,
    shared: Arc<RunShared>,
    rx: Receiver<Cmd>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let name = format!("random-{}", plan.run_id);
    std::thread::Builder::new().name(name).spawn(move || {
        let mut a = Actor {
            plan,
            shared,
            svc: None,
            lock: None,
            view: initial,
            table: ChannelTable::new(),
            started: None,
            ended_at: None,
            stop_outcome: "operator_stop",
            stopped_by: "the operator".into(),
            forwarded: 0,
            alarm_was: false,
            sim_frames: Arc::new(AtomicU64::new(0)),
            record: None,
            record_status: None,
            drive_stream: None,
            run_uid: String::new(),
            utc_offset_s: None,
            last_spectra: None,
            spectra_kept: 0,
        };
        a.run(rx);
    })
}

impl Actor {
    fn run(&mut self, rx: Receiver<Cmd>) {
        loop {
            match (self.view.state, self.view.phase) {
                (State::Running, "sysid") => self.identify(),
                (State::Running, _) | (State::Stopping, _) => {
                    loop {
                        match rx.try_recv() {
                            Ok(cmd) => self.handle(cmd),
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => {
                                self.abandon();
                                return;
                            }
                        }
                    }
                    if matches!(self.view.state, State::Running | State::Stopping) {
                        self.tick();
                    }
                }
                _ => match rx.recv() {
                    Ok(cmd) => self.handle(cmd),
                    Err(_) => {
                        self.abandon();
                        return;
                    }
                },
            }
        }
    }

    /// The service dropped this run: release anything held, the drive first.
    fn abandon(&mut self) {
        if self.svc.is_some() {
            self.shared.signal.store(ABORT, Ordering::SeqCst);
        }
        self.svc = None;
        self.lock = None;
        // The record keeps what it has, `complete: false`: the run did not end.
        self.shared.detach_record();
        self.record = None;
    }

    fn publish(&mut self) {
        if let (Some(status), Some(r)) = (self.record_status.as_ref(), self.view.record.as_mut()) {
            r.problem = status.problem();
        }
        if let Some(t0) = self.started {
            self.view.elapsed_s = Some(self.ended_at.unwrap_or_else(|| t0.elapsed().as_secs_f64()));
        }
        self.shared.publish(self.view.clone());
    }

    fn set(&mut self, state: State, phase: &'static str) {
        if self.view.state != state || self.view.phase != phase {
            if self.view.phase != phase {
                self.shared.add_log_data(
                    "state",
                    format!(
                        "{} to {}",
                        self.view.phase.replace('_', " "),
                        phase.replace('_', " ")
                    ),
                    Some(json!({"from": self.view.phase, "to": phase, "state": state.name()})),
                );
            }
            self.view.state = state;
            self.view.phase = phase;
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Arm { ack, ctx, reply } => {
                let r = self.arm_cmd(ack, ctx);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Start { ctx, reply } => {
                let r = self.start_cmd(ctx);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Proceed { reply } => {
                let r = self.proceed_cmd();
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Level { target, reply } => {
                let r = self.level_cmd(target);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Mode { mode, reply } => {
                let r = self.mode_cmd(mode);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Stop { outcome, by, reply } => {
                self.stop_cmd(outcome, by);
                self.publish();
                if let Some(reply) = reply {
                    let _ = reply.send(Ok(()));
                }
            }
            Cmd::Abort { by, reply } => {
                if self.view.state.is_live() {
                    self.shared.add_log_data(
                        "note",
                        format!("abort requested by {by}"),
                        Some(json!({"action": "abort", "by": by})),
                    );
                    self.stopped_by = by;
                    self.end("operator_abort", "stopped", None, None);
                }
                self.publish();
                let _ = reply.send(Ok(()));
            }
        }
    }

    // ---- arm and start -------------------------------------------------------------

    fn arm_cmd(&mut self, ack: Option<String>, ctx: ArmContext) -> Result<(), MethodError> {
        match self.view.state {
            State::Configured | State::Blocked => {}
            State::Finished => return Err(self.finished_refusal()),
            _ => return Err(self.run_active()),
        }
        if ack.is_some() && self.view.state != State::Blocked {
            return Err(refuse(
                "override_not_needed",
                "The run is not blocked by feasibility; arm without an acknowledgement.",
            ));
        }
        self.arm(ack, ctx)
    }

    fn start_cmd(&mut self, ctx: ArmContext) -> Result<(), MethodError> {
        match self.view.state {
            State::Configured | State::Blocked => self.arm(None, ctx)?,
            State::Armed => {}
            State::Finished => return Err(self.finished_refusal()),
            _ => return Err(self.run_active()),
        }
        self.started = Some(Instant::now());
        let sysid = self.plan.config.control.pretest_level_db;
        self.view.level_db = Some(sysid);
        self.view.live_level_db = Some(sysid);
        self.view.streaming = true;
        self.set(State::Running, "sysid");
        self.shared.add_log_data(
            "note",
            format!(
                "started; system identification at {} against the reference",
                db(sysid)
            ),
            Some(json!({"action": "start", "level_db": sysid})),
        );
        if let Some(r) = self.record.as_ref() {
            let frame = self.shared.station_frame.load(Ordering::Relaxed);
            let mut patch = serde_json::Map::new();
            patch.insert(
                "start".into(),
                rec::start_json(
                    now_unix(),
                    self.utc_offset_s,
                    if frame == NO_FRAME { 0 } else { frame },
                ),
            );
            r.patch_manifest(patch);
        }
        Ok(())
    }

    fn finished_refusal(&self) -> MethodError {
        refuse(
            "run_finished",
            "This run has finished; reset or configure again to run.",
        )
        .with_detail(json!({"outcome": self.view.end.as_ref().map(|e| e.outcome)}))
    }

    fn run_active(&self) -> MethodError {
        refuse(
            "run_active",
            format!(
                "Run {} is {}; stop it first.",
                self.view.run_id,
                self.view.state.name()
            ),
        )
        .with_detail(json!({"run_id": self.view.run_id}))
    }

    /// Feasibility, the station, the arm gate and the grid at the rate the station
    /// programmed. Nothing is driven.
    fn arm(&mut self, ack: Option<String>, ctx: ArmContext) -> Result<(), MethodError> {
        let cfg = self.plan.config.clone();
        let cfg = &cfg;
        let (report, _) = cfg.check(&ctx.table);
        if !report.errors.is_empty() {
            return Err(config_invalid(&report));
        }
        let built = cfg.build(&ctx.table)?;
        // The controller is given the operator's table as it stands: it finds each
        // input's place in the acquired frame without counting drive rows.
        let table = ctx.table.clone();
        let station = station::build(&ctx.station, &*ctx.scanner)?;
        let hardware = station.station.is_hardware();
        let lock = if hardware {
            Some(
                hertzforge_station::lock::acquire(
                    &ctx.data_root,
                    &ctx.bind,
                    &station.provenance.station_label,
                )
                .map_err(|e| match e {
                    hertzforge_station::lock::LockError::Held(h) => refuse(
                        "hardware_busy",
                        format!("Another program holds the NI hardware: {}", h.advice()),
                    )
                    .with_detail(json!({"holder": h.advice()})),
                    hertzforge_station::lock::LockError::Io(m) => arm_refused(m),
                })?,
            )
        } else {
            None
        };
        let class = station.station;
        let provenance = station.provenance.clone();
        let caps = station.backend.caps();
        // The streams are taken where the station's blocks pass (record::tap): the
        // inputs as read and the drive as the device accepted it. Their buffers are
        // made here, before anything runs.
        let buffers = if cfg.record_streams {
            rec::tap_buffers(cfg.fs_hz, self.plan.record.stream_buffer_s)
        } else {
            1
        };
        let (inputs_tap, inputs_link) = tap::pair(
            rec::input_rows(&table).len(),
            rec::TAP_BLOCK_FRAMES,
            buffers,
        );
        let (drives_tap, drives_link) = tap::pair(
            rec::drive_rows(&table).len(),
            rec::TAP_BLOCK_FRAMES,
            buffers,
        );
        let recorded = Recording::wrap(
            station.backend,
            inputs_tap,
            drives_tap,
            self.shared.station_frame.clone(),
        );
        let backend = Interruptible::wrap(recorded, self.shared.signal.clone());
        let mut svc = RandomService::new(
            built.test,
            table.clone(),
            built.loop_cfg,
            backend,
            class,
            self.plan.capture_root.clone(),
        )
        .map_err(arm_refused)?;
        svc.set_review_after_sysid(cfg.control.review_after_sysid);
        if !hardware {
            svc.set_block_observer(Some(pacer(
                self.sim_frames.clone(),
                cfg.fs_hz,
                self.plan.sim_speed,
            )));
        }
        let feas = svc.assess_feasibility().map_err(arm_refused)?;
        self.view.feasibility = Some(verdict(&feas, cfg.feasibility.is_some()));
        self.forward_log(&svc);
        if svc.state() == RunState::FeasibilityBlocked {
            match &ack {
                Some(a) => {
                    svc.arm_with_override(a).map_err(arm_refused)?;
                    self.shared.add_log_data(
                        "override",
                        format!("operator acknowledgement: {a}"),
                        Some(json!({"what": "feasibility_exceeded", "acknowledgement": a})),
                    );
                }
                None => {
                    self.set(State::Blocked, "feasibility_blocked");
                    let feasibility = self
                        .view
                        .feasibility
                        .as_ref()
                        .map(|(v, m)| json!({"verdict": v, "message": m}));
                    return Err(refuse(
                        "feasibility_exceeded",
                        "Feasibility exceeded: the profile asks more of the shaker than its \
                         ratings. Review it, or arm with an acknowledgement.",
                    )
                    .with_detail(json!({ "feasibility": feasibility })));
                }
            }
        } else {
            svc.arm().map_err(arm_refused)?;
        }
        self.forward_log(&svc);
        let grid = Grid::of(svc.loop_config(), true);
        let band = (
            cfg.rows.first().map_or(0.0, |r| r.freq_hz),
            cfg.rows.last().map_or(0.0, |r| r.freq_hz),
        );
        let mut r = Report::default();
        check_grid(&mut r, band, &grid);
        if let Some(item) = r.errors.first() {
            return Err(arm_refused(&item.message));
        }
        let lines = super::view::lines_on(svc.loop_config(), cfg);
        let limits = limit_lines(svc.loop_config(), cfg);

        // The run's record, before any sample: refused here, the arm is refused and
        // the station released (nothing has been driven).
        let armed_unix = now_unix();
        self.utc_offset_s = rec::utc_offset_s();
        self.run_uid = rec::run_uid(armed_unix, &self.plan.run_id);
        let root = self
            .plan
            .capture_root
            .clone()
            .unwrap_or_else(hertzforge_capture::default_data_root);
        let streams_off = if !cfg.record_streams {
            Some("switched off in the setup".to_owned())
        } else {
            match hertzforge_capture::free_space_bytes_for(&root) {
                Ok(free) if free < self.plan.record.free_space_floor_bytes => Some(format!(
                    "the disk had {} MB free, under the {} MB kept free",
                    free / 1_000_000,
                    self.plan.record.free_space_floor_bytes / 1_000_000
                )),
                _ => None,
            }
        };
        let (check, derived) = cfg.check(&table);
        let derived = check.errors.is_empty().then_some(derived).flatten();
        let law = json!({
            "id": svc.law_name(), "name": svc.law_name(), "version": null, "sha256": null,
            "builtin": true, "params": {},
            "guards": serde_json::to_value(&svc.config().law_guards).unwrap_or(Value::Null),
        });
        let facts = rec::ArmFacts {
            run_id: &self.plan.run_id,
            run_uid: &self.run_uid,
            cfg,
            table: &table,
            revision: ctx.revision,
            plan: &ctx.station,
            class,
            provenance: &provenance,
            devices: &ctx.devices,
            caps: &caps,
            loop_cfg: svc.loop_config(),
            grid: &grid,
            derived: derived.as_ref(),
            feasibility: self.view.feasibility.as_ref(),
            law,
            armed_unix,
            utc_offset_s: self.utc_offset_s,
            streams_off: streams_off.clone(),
            log: Vec::new(),
        };
        let begin = rec::begin(&facts, root, (inputs_link, drives_link));
        let drive_stream = begin.streams.iter().position(|s| s.name == "drives");
        let shared = self.shared.clone();
        let problem: ProblemFn = Box::new(move |words: String, data: Value| {
            shared.add_log_data("record", words.clone(), Some(data));
            let mut v = (*shared.view()).clone();
            if let Some(r) = v.record.as_mut() {
                r.problem = Some(words);
                shared.publish(v);
            }
        });
        let record = RunRecord::open(begin, &self.plan.record, problem).map_err(|e| {
            refuse(
                "record_unwritable",
                format!(
                    "The run's record could not be written, so the run did not arm: {}                      ({}). Choose a data folder that can be written.",
                    e.path.display(),
                    e.reason
                ),
            )
            .with_detail(json!({"path": e.path.display().to_string(), "reason": e.reason}))
        })?;
        self.shared.attach_record(record.log());
        if let Some(why) = &streams_off {
            self.shared.add_log_data(
                "record",
                format!("the run's streams are not recorded: {why}"),
                Some(json!({"stream": null, "failed": why})),
            );
        }
        for (name, d, arrays) in std::iter::once(rec::reference_result(&lines))
            .chain(limits.iter().map(|l| rec::limit_result(l, &table)))
        {
            record.put_result(&name, d, arrays, None);
        }
        self.view.record = Some(RecordRef {
            folder: record.folder().display().to_string(),
            streams: streams_off.is_none(),
            problem: None,
        });
        self.record_status = Some(record.status());
        self.drive_stream = drive_stream;
        self.record = Some(record);

        self.view.lines = Some(Arc::new(lines));
        self.view.limits = Arc::new(limits);
        self.view.grid = grid.clone();
        self.view.clock = Some("shared");
        self.view.station = StationRef {
            kind: if matches!(ctx.station, StationPlan::Sim { .. }) {
                "sim"
            } else {
                "ni"
            },
            class: class_name(class),
            devices: provenance
                .device_ids
                .iter()
                .cloned()
                .zip(provenance.serials.iter().cloned())
                .collect(),
        };
        self.view.mode = cfg.mode;
        self.view.channels = live_channels(&table);
        self.table = table;
        if let Ok(mut r) = self.shared.ramp.lock() {
            *r = Some(svc.drive_ramp());
        }
        self.svc = Some(svc);
        self.lock = lock;
        self.set(State::Armed, "armed");
        self.shared.add_log_data(
            "station",
            format!(
                "armed on {} at {} Hz, {} Hz lines, {} in the band, shared clock",
                class_name(class),
                super::config::round(grid.fs_hz, 3),
                super::config::round(grid.df_hz, 4),
                grid.lines
            ),
            Some(
                json!({"action": "arm", "class": class_name(class), "fs_hz": grid.fs_hz,
                        "df_hz": grid.df_hz, "lines": grid.lines}),
            ),
        );
        Ok(())
    }

    // ---- system identification and the gate ------------------------------------------

    fn identify(&mut self) {
        let segments = self.plan.config.control.pretest_averages.max(1) as usize;
        let Some(svc) = self.svc.as_mut() else {
            self.fault("the run lost its station before system identification");
            return;
        };
        let r = svc.run_sysid(segments);
        let tripped = svc.state() == RunState::Aborted;
        self.forward_log_now();
        match r {
            Err(ServiceError::SysIdLevelUnreachable {
                level_db,
                target_rms,
                reached_rms,
                drive_vrms,
                ceiling_vrms,
                probes,
            }) => {
                if self.interrupted() {
                    return;
                }
                let unit = self.control_unit();
                let below = num(super::config::round(-level_db, 1));
                let first = if drive_vrms >= ceiling_vrms * (1.0 - 1e-9) && reached_rms < target_rms
                {
                    format!(
                        "System identification could not reach its level: at its largest drive, \
                         {ceiling_vrms:.3} V rms, the control response was {reached_rms:.4} \
                         {unit} rms, short of the {target_rms:.4} {unit} rms that {below} dB below \
                         the reference asks for. Raise the largest identification drive, or check \
                         the drive and the sensor."
                    )
                } else {
                    format!(
                        "System identification did not settle on its level: after {probes} \
                         drives the control response was {reached_rms:.4} {unit} rms at \
                         {drive_vrms:.3} V rms, against the {target_rms:.4} {unit} rms that \
                         {below} dB below the reference asks for. Check the drive and the \
                         sensor."
                    )
                };
                self.end(
                    "sysid_level_unreachable",
                    "stopped",
                    None,
                    Some(format!("{first} The run never drove at test level.")),
                );
                return;
            }
            Err(e) => {
                self.after_error(&e.to_string(), "system identification failed");
                return;
            }
            Ok(()) => {}
        }
        if self.interrupted() {
            return;
        }
        if tripped {
            // A protection tripped during identification (it runs under the same
            // aborts as a control loop): end on the trip, as a loop would.
            self.finish_from_controller();
            return;
        }
        if let Some(svc) = self.svc.as_ref() {
            let (lo, hi) = svc.loop_config().band_bins();
            let curve = svc.coherence_curve();
            self.view.coherence =
                (curve.len() >= hi && lo < hi).then(|| Arc::new(curve[lo..hi].to_vec()));
            if let Some(r) = self.record.as_ref() {
                for (name, d, arrays) in rec::identification_results(
                    svc.frf_complex(),
                    curve,
                    svc.loop_config(),
                    &self.table,
                ) {
                    r.put_result(&name, d, arrays, None);
                }
                if let Some(k) = self.drive_stream {
                    let mut patch = serde_json::Map::new();
                    patch.insert(
                        "pipeline_lag_frames".into(),
                        json!({"frames": svc.sysid_lag_frames(), "how": "measured",
                               "by": "system identification: drive to control response"}),
                    );
                    r.patch_stream(k, patch);
                }
            }
        }
        self.set(State::Running, "coherence_gate");
        self.publish();
        let Some(svc) = self.svc.as_mut() else { return };
        let gate = svc.coherence_gate();
        let coherence = svc.band_coherence();
        let state = svc.state();
        self.forward_log_now();
        match gate {
            Err(e) => self.after_error(&e.to_string(), "the coherence gate failed"),
            Ok(_) if state == RunState::CoherenceHold => {
                let floor = self.plan.config.control.coherence_floor;
                self.end(
                    "held",
                    "coherence_hold",
                    None,
                    Some(format!(
                        "Coherence gate held: band coherence {coherence:.2} is below the floor \
                         {floor:.2}; the run never drove at test level."
                    )),
                );
            }
            Ok(_) if state == RunState::AwaitingReview => {
                self.view.level_db = None;
                self.view.live_level_db = None;
                self.set(State::Review, "awaiting_review");
                self.publish();
            }
            Ok(_) => {
                if self.shared.signal.load(Ordering::SeqCst) == STOP {
                    self.end(self.stop_outcome, "stopped", None, None);
                    return;
                }
                self.engaged();
            }
        }
    }

    /// Ended by the operator's signal while a long call ran.
    fn interrupted(&mut self) -> bool {
        match self.shared.signal.load(Ordering::SeqCst) {
            ABORT => {
                self.end("operator_abort", "stopped", None, None);
                true
            }
            STOP => {
                self.end(self.stop_outcome, "stopped", None, None);
                true
            }
            _ => false,
        }
    }

    fn after_error(&mut self, words: &str, what: &str) {
        if self.interrupted() {
            return;
        }
        if let Some(svc) = self.svc.as_mut() {
            svc.fault_from_loop(words);
        }
        self.fault(&format!("{what}: {words}"));
    }

    fn fault(&mut self, words: &str) {
        self.forward_log_now();
        self.end(
            "fault",
            "aborted",
            None,
            Some(format!("The control loop stopped on a fault: {words}")),
        );
    }

    /// The unit of the first control row, for a message.
    fn control_unit(&self) -> String {
        self.table
            .channels
            .iter()
            .find(|c| c.enabled && c.role == ChannelRole::Control)
            .map_or_else(|| "g".to_owned(), |c| unit_name(&c.units).to_string())
    }

    fn engaged(&mut self) {
        let phase = self.svc.as_ref().map_or("engaged", driving_phase);
        self.set(State::Running, phase);
        self.refresh_status();
        self.publish();
    }

    // ---- the control loop ---------------------------------------------------------

    fn tick(&mut self) {
        if self.shared.signal.load(Ordering::SeqCst) == ABORT {
            self.end("operator_abort", "stopped", None, None);
            return;
        }
        let Some(svc) = self.svc.as_mut() else {
            self.fault("the run lost its station");
            return;
        };
        let base = svc.schedule_level_db();
        let r = svc.loop_tick();
        match r {
            Ok(more) => {
                self.view.loops += 1;
                self.view.drive_level_db = Some(base);
                // The controller started its own controlled stop (its control law
                // stopped producing a usable drive, or the schedule held full level
                // for its duration): show the ramp-down as one.
                let law_stop = more
                    && self.view.state == State::Running
                    && self
                        .svc
                        .as_ref()
                        .is_some_and(|s| law_fault_trip(s).is_some() || s.schedule_complete());
                if law_stop {
                    if let Some(svc) = self.svc.as_ref() {
                        self.view.level_db = Some(svc.commanded_level_db());
                    }
                    self.set(State::Stopping, "ramping_down");
                }
                self.after_loop();
                if !more {
                    self.finish_from_controller();
                } else {
                    self.publish();
                }
            }
            Err(e) => self.after_error(&e.to_string(), "control loop fault"),
        }
    }

    /// The controller ended the run on this loop: the schedule finished, a controlled
    /// stop reached its floor, or a protection tripped.
    fn finish_from_controller(&mut self) {
        let Some(svc) = self.svc.as_ref() else { return };
        // The controller stopped the run itself because its control law stopped
        // producing a usable drive: a controlled stop with a trip record. It wins
        // over an operator stop that arrived during its ramp-down.
        if let (RunState::Stopped, Some(t)) = (svc.state(), law_fault_trip(svc)) {
            let (json, message) = trip_facts(&t, false);
            self.shared
                .add_log_data("trip", message.clone(), Some(json.clone()));
            self.end("law_fault", "stopped", Some(json), Some(message));
            return;
        }
        match svc.state() {
            RunState::Aborted => {
                let trip = svc.trip().cloned();
                match trip {
                    Some(t) => {
                        let during_sysid = self.view.phase == "sysid";
                        let (json, message) = trip_facts(&t, during_sysid);
                        self.shared
                            .add_log_data("trip", message.clone(), Some(json.clone()));
                        self.end("aborted", "aborted", Some(json), Some(message));
                    }
                    None => self.fault("the controller ended the run without a trip record"),
                }
            }
            // The schedule held full level for its duration and the shutdown ramp
            // has taken the drive down: a completion, whoever pressed Stop during
            // that ramp.
            RunState::Stopped if svc.schedule_complete() => {
                let s = self.plan.config.full_level_s;
                self.end(
                    "completed",
                    "stopped",
                    None,
                    Some(format!("Test complete: {} s held at full level.", num(s))),
                );
            }
            RunState::Stopped if self.view.state == State::Stopping => {
                self.end(self.stop_outcome, "stopped", None, None);
            }
            // The ramp-down started from the socket's thread arrived before the run
            // thread took the queued stop request: end as that request says.
            RunState::Stopped if svc.stop_requested_db().is_some() => {
                let pending = self.shared.pending_stop.lock().ok().and_then(|p| p.clone());
                if let Some((outcome, by)) = pending {
                    self.stop_outcome = outcome;
                    self.stopped_by = by;
                }
                self.end(self.stop_outcome, "stopped", None, None);
            }
            RunState::Stopped => {
                let s = self.plan.config.full_level_s;
                self.end(
                    "completed",
                    "stopped",
                    None,
                    Some(format!("Test complete: {} s held at full level.", num(s))),
                );
            }
            other => self.fault(&format!(
                "the control loop ended in state {}",
                crate::sim::phase_name(other)
            )),
        }
    }

    /// Read the controller's status after a loop.
    fn refresh_status(&mut self) {
        let Some(svc) = self.svc.as_ref() else { return };
        let st = svc.status();
        let driving = matches!(st.state, RunState::Engaged | RunState::FullLevel);
        self.view.live_level_db = Some(st.current_level_db);
        self.view.level_db = Some(svc.commanded_level_db());
        self.view.mode = st.run_mode;
        self.view.at_level_s = driving.then_some(st.elapsed_at_level_s);
        self.view.remaining_at_level_s = if driving {
            st.remaining_at_level_s
        } else {
            None
        };
        // The time the schedule holds this level: what is left plus what is done
        // (the dwell, or the full level time), in auto only.
        self.view.level_s = match (driving, st.remaining_at_level_s) {
            (true, Some(left)) if st.state == RunState::FullLevel => Some(
                self.plan
                    .config
                    .full_level_s
                    .max(left + st.elapsed_at_level_s),
            ),
            (true, Some(left)) => Some(left + st.elapsed_at_level_s),
            _ => None,
        };
        self.view.reference_rms_g = Some(st.target_grms);
        self.view.control_rms_g = st.measured_grms;
        if self.view.state == State::Running {
            self.view.phase = driving_phase(svc);
        }
    }

    fn after_loop(&mut self) {
        self.refresh_status();
        let Some(svc) = self.svc.as_ref() else { return };
        let st = svc.status();
        let rep = svc.report_data();
        let (lo, hi) = svc.loop_config().band_bins();
        let df = svc.realized_df_hz();
        let band = |psd: &[f64]| -> Vec<f64> {
            (lo..hi)
                .map(|k| psd.get(k).copied().unwrap_or(0.0).max(0.0))
                .collect()
        };
        self.view.control_psd = rep
            .final_composite_psd
            .as_ref()
            .map(|c| Arc::new(band(&c.psd)));
        self.view.drive_psd = rep.final_drive_psd.as_ref().map(|d| Arc::new(band(&d.psd)));
        self.view.drive_rms_v = rep.final_drive_psd.as_ref().map(|d| {
            (d.psd.iter().filter(|p| **p > 0.0).sum::<f64>() * d.df_hz)
                .max(0.0)
                .sqrt()
        });
        self.view.lines_alarm = rep.final_alarm_lines;
        self.view.lines_abort = rep.final_abort_lines;

        // Each enabled input's rms over the band, in its own unit: the controller's
        // own per-channel spectrum for a control row, the same estimate of the last
        // acquired segment for any other input.
        let rms_of = |psd: &[f64]| -> f64 {
            ((lo..hi)
                .map(|k| psd.get(k).copied().unwrap_or(0.0).max(0.0))
                .sum::<f64>()
                * df)
                .sqrt()
        };
        let controls = &rep.settings.control_indices;
        let segment = svc.last_segment();
        let loop_cfg = svc.loop_config().clone();
        let inputs: Vec<u32> = self
            .table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .map(|c| c.index)
            .collect();
        // Every input's PSD on the band's lines and its samples, kept for the
        // screens that plot them (schema/PROTOCOL.md, "Live Data Subscriptions").
        let mut data = Inputs {
            psd: Vec::with_capacity(self.view.channels.len()),
            time: Vec::with_capacity(self.view.channels.len()),
            fs_hz: segment.map_or(svc.loop_fs_hz(), |(_, _, fs)| fs),
        };
        for ch in self.view.channels.iter_mut() {
            ch.overload = st
                .overload_latches
                .iter()
                .any(|(i, latched)| *i == ch.index && *latched);
            let samples: Option<Vec<f32>> =
                match (segment, inputs.iter().position(|i| *i == ch.index)) {
                    (Some((samples, n_ch, _)), Some(slot)) => Some(
                        samples
                            .iter()
                            .skip(slot)
                            .step_by(n_ch.max(1))
                            .copied()
                            .collect(),
                    ),
                    _ => None,
                };
            let mut psd_band = None;
            if let Some(pos) = controls.iter().position(|i| *i == ch.index) {
                ch.rms = rep.final_per_channel_psds.get(pos).map(|l| rms_of(&l.psd));
                psd_band = rep.final_per_channel_psds.get(pos).map(|l| band(&l.psd));
            } else if let (Some(signal), Some((_, _, fs))) = (samples.as_ref(), segment) {
                let signal: Vec<f64> = signal.iter().map(|x| f64::from(*x)).collect();
                let (_, psd) = hertzforge_dsp::spectrum::psd_welch(
                    &signal,
                    fs,
                    loop_cfg.nfft,
                    loop_cfg.overlap,
                );
                ch.rms = (!psd.is_empty()).then(|| rms_of(&psd));
                psd_band = (!psd.is_empty()).then(|| band(&psd));
            }
            data.psd.push(psd_band);
            data.time.push(samples.unwrap_or_default());
        }
        // The loop's spectra, kept for the record at a level change and at the end.
        let drive_row = rec::drive_rows(&self.table).first().map(|c| c.index);
        self.last_spectra = rep.final_composite_psd.as_ref().map(|c| Spectra {
            level_db: self.view.drive_level_db.unwrap_or(st.current_level_db),
            loop_index: self.view.loops,
            first_hz: lo as f64 * df,
            df_hz: df,
            controls: controls
                .iter()
                .zip(rep.final_per_channel_psds.iter())
                .map(|(i, l)| (*i, band(&l.psd)))
                .collect(),
            composite: band(&c.psd),
            inputs: self
                .view
                .channels
                .iter()
                .zip(data.psd.iter())
                .map(|(ch, p)| (ch.index, p.clone()))
                .collect(),
            drive: drive_row.zip(rep.final_drive_psd.as_ref().map(|d| band(&d.psd))),
        });
        self.view.inputs = Some(Arc::new(data));
        let mut alarms = Vec::new();
        if st.alarm_active && rep.final_alarm_lines > 0 {
            let worst = svc.log().events().iter().rev().find_map(|e| match e.kind {
                EventKind::EnvelopeAlarm { worst_db, .. } => Some(worst_db),
                _ => None,
            });
            alarms.push(Alarm::Envelope {
                lines: rep.final_alarm_lines,
                worst_db: finite_db(worst.unwrap_or(0.0)),
            });
        }
        for c in &self.view.channels {
            if c.overload {
                alarms.push(Alarm::Overload { channel: c.index });
            }
        }
        self.view.alarm = !alarms.is_empty();
        self.view.alarms = alarms;
        self.forward_log_now();
    }

    // ---- level, mode, review, stop -------------------------------------------------

    fn proceed_cmd(&mut self) -> Result<(), MethodError> {
        if self.view.state != State::Review {
            return Err(wrong_state(&self.view, "Proceeding"));
        }
        let svc = self
            .svc
            .as_mut()
            .ok_or_else(|| wrong_state(&self.view, "Proceeding"))?;
        svc.proceed_after_review()
            .map_err(|e| refuse("wrong_state", e.to_string()))?;
        self.shared.add_log_data(
            "note",
            "the operator reviewed system identification and proceeded",
            Some(json!({"action": "proceed"})),
        );
        self.forward_log_now();
        self.engaged();
        Ok(())
    }

    fn level_cmd(&mut self, target: LevelTarget) -> Result<f64, MethodError> {
        let driving = self.view.state == State::Running
            && matches!(self.view.phase, "engaged" | "full_level");
        if self.view.mode != RunMode::Manual {
            return Err(refuse(
                "manual_mode_required",
                "Level changes need the manual ladder; switch the ladder to manual first.",
            )
            .with_detail(json!({"mode": "auto"})));
        }
        if !driving {
            return Err(wrong_state(&self.view, "A level change"));
        }
        let levels = self.view.levels_db.clone();
        let svc = self
            .svc
            .as_mut()
            .ok_or_else(|| wrong_state(&self.view, "A level change"))?;
        let r =
            match target {
                LevelTarget::Delta(d) => svc.step_level(d),
                LevelTarget::To(db) => match levels.iter().position(|l| (l - db).abs() < 1e-9) {
                    Some(i) => svc.jump_to_table_level(i),
                    None => return Err(MethodError::named(
                        ErrorCode::BadRequest,
                        "not_a_rung",
                        format!(
                            "{} dB is not a level of the schedule ({} dB).",
                            num(db),
                            levels
                                .iter()
                                .map(|l| num(*l).to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )
                    .with_detail(
                        json!({"levels_db": levels.iter().map(|l| num(*l)).collect::<Vec<_>>()}),
                    )),
                },
            };
        let level = r.map_err(|e| refuse("wrong_state", e.to_string()))?;
        self.forward_log_now();
        self.refresh_status();
        Ok(level)
    }

    fn mode_cmd(&mut self, mode: RunMode) -> Result<(), MethodError> {
        match self.view.state {
            State::Finished => Err(wrong_state(&self.view, "Changing the ladder mode")),
            State::Configured | State::Blocked => {
                self.plan.config.mode = mode;
                self.view.mode = mode;
                Ok(())
            }
            _ => {
                let svc = self
                    .svc
                    .as_mut()
                    .ok_or_else(|| wrong_state(&self.view, "Changing the ladder mode"))?;
                svc.set_run_mode(mode)
                    .map_err(|e| refuse("wrong_state", e.to_string()))?;
                self.view.mode = mode;
                self.shared.add_log_data(
                    "note",
                    format!("ladder mode {}", super::view::mode_name(mode)),
                    Some(json!({"action": "mode", "mode": super::view::mode_name(mode)})),
                );
                self.forward_log_now();
                if self.view.state == State::Running {
                    self.refresh_status();
                }
                Ok(())
            }
        }
    }

    fn stop_cmd(&mut self, outcome: &'static str, by: String) {
        if !self.view.state.is_live() || self.view.state == State::Stopping {
            return;
        }
        self.stop_outcome = outcome;
        self.stopped_by = by;
        self.shared.add_log_data(
            "note",
            if outcome == "safe_stop" && self.stopped_by == SHUTDOWN_BY {
                "the control service is shutting down; stopping the run with its shutdown ramp"
                    .to_owned()
            } else if outcome == "safe_stop" {
                format!(
                    "nobody held control for {} s; stopping the run with its shutdown ramp",
                    num(self.plan.timeout_s)
                )
            } else {
                format!("stop requested by {}", self.stopped_by)
            },
            Some(
                json!({"action": if outcome == "safe_stop" { "safe_stop" } else { "stop" },
                        "by": self.stopped_by}),
            ),
        );
        match self.view.state {
            State::Armed | State::Review => self.end(outcome, "stopped", None, None),
            State::Running if matches!(self.view.phase, "engaged" | "full_level") => {
                let Some(svc) = self.svc.as_mut() else { return };
                let r = svc.request_stop();
                let stopped = svc.state() == RunState::Stopped;
                let commanded = svc.commanded_level_db();
                self.forward_log_now();
                if r.is_err() || stopped {
                    self.end(outcome, "stopped", None, None);
                } else {
                    // The commanded level is where the ramp-down is headed.
                    self.view.level_db = Some(commanded);
                    self.set(State::Stopping, "ramping_down");
                }
            }
            // System identification and the gate end through the station signal,
            // set by the service before this request was queued.
            _ => {}
        }
    }

    // ---- the end ------------------------------------------------------------------

    /// End the run, release the station, and say how it ended.
    fn end(
        &mut self,
        outcome: &'static str,
        phase: &'static str,
        trip: Option<Value>,
        message: Option<String>,
    ) {
        if self.view.state == State::Finished {
            return;
        }
        let elapsed = self.started.map(|t| t.elapsed().as_secs_f64());
        self.ended_at = elapsed;
        if matches!(outcome, "operator_stop" | "operator_abort") {
            if let Some(by) = self.shared.ended_by.lock().ok().and_then(|b| b.clone()) {
                self.stopped_by = by;
            }
        }
        let was_driving = matches!(self.view.state, State::Running | State::Stopping);
        // The level where the end began (2026-11-01, bench finding 3): a controlled
        // stop's ramp starts from it and a trip records it; the level the drive
        // reached by the end of a ramp-down is not where the operator stopped.
        let requested = match outcome {
            "operator_stop" | "safe_stop" | "completed" => self
                .svc
                .as_ref()
                .and_then(|s| s.stop_requested_db())
                .or_else(|| self.shared.requested_level.lock().ok().and_then(|l| *l)),
            "operator_abort" => self.shared.requested_level.lock().ok().and_then(|l| *l),
            "aborted" | "law_fault" => trip
                .as_ref()
                .and_then(|t| t.get("at_level_db"))
                .and_then(Value::as_f64),
            _ => None,
        };
        let level = if was_driving {
            requested.or(self.view.live_level_db)
        } else {
            None
        };
        let at = format!(
            "{} after {:.1} s",
            level.map_or_else(|| "before driving".to_owned(), db),
            elapsed.unwrap_or(0.0)
        );
        let message = message.unwrap_or_else(|| match outcome {
            "operator_stop" => format!("Stopped by {} at {at}.", self.stopped_by),
            "operator_abort" => format!("Aborted by {} at {at}.", self.stopped_by),
            "safe_stop" if self.stopped_by == SHUTDOWN_BY => format!(
                "The control service was shut down, so it stopped the run with its shutdown ramp, \
                 from {at}."
            ),
            "safe_stop" => format!(
                "Nobody held control for {} s, so the service stopped the run with its shutdown \
                 ramp, from {at}.",
                num(self.plan.timeout_s)
            ),
            _ => "The run ended.".to_owned(),
        });
        self.forward_log_now();
        // A protection trip and an operator abort stop the station with its own
        // abort stop; every other end takes the controlled stop.
        if matches!(outcome, "aborted" | "operator_abort" | "fault") {
            self.shared.signal.store(ABORT, Ordering::SeqCst);
        }
        self.svc = None;
        if let Ok(mut r) = self.shared.ramp.lock() {
            *r = None;
        }
        self.lock = None;
        self.view.end = Some(End {
            outcome,
            message: message.clone(),
            at_s: elapsed,
            level_db: level,
            trip,
        });
        self.view.level_db = None;
        self.view.at_level_s = None;
        self.view.remaining_at_level_s = None;
        self.set(State::Finished, phase);
        self.view.live_level_db = None;
        self.view.drive_now_db = None;
        self.shared.add_log_data(
            "note",
            message,
            Some(json!({"outcome": outcome, "by": self.stopped_by})),
        );
        self.publish();
        self.finish_record();
    }

    /// Keep the last loop's spectra in the record, as the repeating results'
    /// next record.
    fn keep_spectra(&mut self, why: &str) {
        let (Some(r), Some(s)) = (self.record.as_ref(), self.last_spectra.as_ref()) else {
            return;
        };
        let frame = self.shared.station_frame.load(Ordering::Relaxed);
        let line = json!({"index": self.spectra_kept, "why": why, "level_db": s.level_db,
                          "loop": s.loop_index,
                          "frame": if frame == NO_FRAME { Value::Null } else { Value::from(frame) },
                          "at_unix": now_unix()});
        for (name, d, arrays) in rec::spectra_results(s, &self.table) {
            r.put_result(&name, d, arrays, Some(line.clone()));
        }
        self.spectra_kept += 1;
    }

    /// End the record: the last spectra, `end.json` with the run's end as its
    /// state reports it, then `complete`.
    fn finish_record(&mut self) {
        if self.record.is_none() {
            return;
        }
        self.keep_spectra("end");
        self.shared.detach_record();
        let Some(r) = self.record.take() else { return };
        let state = super::view::state_json(Some(&self.view));
        let frame = self.shared.station_frame.load(Ordering::Relaxed);
        let mut end = serde_json::Map::new();
        end.insert("run_uid".into(), Value::from(self.run_uid.as_str()));
        end.insert("end".into(), state["end"].clone());
        end.insert("at_unix".into(), Value::from(now_unix()));
        end.insert(
            "station_frames".into(),
            if frame == NO_FRAME {
                Value::from(0)
            } else {
                Value::from(frame)
            },
        );
        end.insert(
            "summary".into(),
            json!({"loops": self.view.loops, "spectra_kept": self.spectra_kept,
                   "elapsed_s": self.ended_at}),
        );
        r.finish(end);
    }

    // ---- the controller's log -----------------------------------------------------

    fn forward_log_now(&mut self) {
        if let Some(svc) = self.svc.take() {
            self.forward_log(&svc);
            self.svc = Some(svc);
        }
    }

    /// Copy the controller's new log events into the run log.
    fn forward_log(&mut self, svc: &RandomService) {
        let events = svc.log().events();
        let mut level_changed = false;
        for e in events.iter().skip(self.forwarded) {
            let entry: Option<(&'static str, String, Option<Value>)> = match &e.kind {
                // The service logs the protocol's own state changes.
                EventKind::StateChange { .. } => None,
                EventKind::Feasibility { overall, .. } => Some((
                    "feasibility",
                    format!("feasibility: {overall}"),
                    Some(json!({"verdict": overall.to_string()})),
                )),
                EventKind::OverrideAck { .. } => None,
                EventKind::StationAdopted {
                    requested_fs_hz,
                    actual_fs_hz,
                    df_hz,
                    control_lines,
                    ..
                } => Some((
                    "station",
                    format!(
                        "the station programmed {actual_fs_hz} Hz for the {requested_fs_hz} Hz \
                         asked; {df_hz:.4} Hz lines, {control_lines} in the band"
                    ),
                    Some(
                        json!({"requested_fs_hz": requested_fs_hz, "fs_hz": actual_fs_hz,
                                "df_hz": df_hz, "lines": control_lines}),
                    ),
                )),
                EventKind::CoherenceGate {
                    band_coherence,
                    floor,
                    passed,
                } => Some((
                    "gate",
                    format!(
                        "coherence gate {}: band coherence {band_coherence:.3}, floor {floor:.2}",
                        if *passed { "passed" } else { "held" }
                    ),
                    Some(json!({"band_coherence": band_coherence, "floor": floor,
                                "passed": passed})),
                )),
                EventKind::LevelChange { to_db, auto, .. } => {
                    level_changed = true;
                    Some((
                        "level",
                        format!(
                            "level {} ({})",
                            db(*to_db),
                            if *auto { "schedule" } else { "operator" }
                        ),
                        Some(json!({"level_db": to_db,
                                    "by": if *auto { "schedule" } else { "operator" }})),
                    ))
                }
                EventKind::EnvelopeAlarm { lines, worst_db } => {
                    if self.alarm_was {
                        None
                    } else {
                        Some((
                            "alarm",
                            format!(
                                "{lines} lines outside the alarm band, worst {}",
                                db(finite_db(*worst_db))
                            ),
                            Some(json!({"kind": "envelope", "lines": lines,
                                        "worst_db": num(finite_db(*worst_db))})),
                        ))
                    }
                }
                EventKind::OverloadLatch { channel_index } => Some((
                    "overload",
                    format!(
                        "channel {channel_index} reached the top of its range (90 % of its range \
                         in the run, or 98 % at the station)"
                    ),
                    Some(json!({"channel": channel_index})),
                )),
                // The service logs the trip with its end message.
                EventKind::Trip { .. } => None,
                EventKind::Note { text } => Some(("note", text.clone(), None)),
                EventKind::CaptureStart { run_dir, .. } => {
                    Some(("note", format!("recording to {run_dir}"), None))
                }
                EventKind::CaptureStop { .. } | EventKind::Snapshot { .. } => None,
            };
            if let Some((kind, text, data)) = entry {
                self.shared.add_log_data(kind, text, data);
            }
        }
        self.forwarded = events.len();
        self.alarm_was = svc.status().alarm_active;
        if level_changed {
            // The spectra the loop ran at before the change.
            self.keep_spectra("level");
        }
    }
}

/// The phase of a driving run: `full_level` once the loop runs at full level, and
/// `engaged` before, including while a level change toward full level ramps (the
/// controller's own state turns full level when the change is commanded).
/// The run's trip record when it is the law-fault controlled stop.
fn law_fault_trip(svc: &RandomService) -> Option<hertzforge_random::TripRecord> {
    svc.trip()
        .filter(|t| matches!(t.kind, TripKind::LawFault { .. }))
        .cloned()
}

fn driving_phase(svc: &RandomService) -> &'static str {
    let arrived = (svc.schedule_level_db() - svc.commanded_level_db()).abs() < 0.05;
    match svc.state() {
        RunState::FullLevel if arrived => "full_level",
        _ => "engaged",
    }
}

fn db(x: f64) -> String {
    format!("{}{:.1} dB", if x >= 0.0 { "+" } else { "" }, x)
}

fn finite_db(x: f64) -> f64 {
    if x.is_finite() {
        x
    } else if x > 0.0 {
        300.0
    } else {
        -300.0
    }
}

/// `config_invalid` (or `table_invalid` for the table's items) from a report.
pub fn config_invalid(report: &Report) -> MethodError {
    MethodError::named(
        ErrorCode::BadRequest,
        "config_invalid",
        format!(
            "The configuration has {} error{}.",
            report.errors.len(),
            if report.errors.len() == 1 { "" } else { "s" }
        ),
    )
    .with_detail(json!({
        "errors": report.errors.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
        "warnings": report.warnings.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
    }))
}

/// The feasibility verdict in words.
fn verdict(r: &hertzforge_shaker_db::FeasibilityReport, given: bool) -> (&'static str, String) {
    use hertzforge_shaker_db::Verdict;
    let items = |want: Verdict| -> String {
        r.items
            .iter()
            .filter(|i| i.verdict == want)
            .map(|i| i.label.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match r.overall {
        Verdict::NotEvaluated if !given => (
            "not_evaluated",
            "No shaker was given, so the profile was not checked against shaker ratings.".into(),
        ),
        Verdict::NotEvaluated => (
            "not_evaluated",
            "The shaker record lacks the ratings to check this profile against.".into(),
        ),
        Verdict::Pass => ("pass", "The profile is within the shaker's ratings.".into()),
        Verdict::Advisory => (
            "advisory",
            format!(
                "The profile is within the shaker's ratings, with advisories: {}.",
                items(Verdict::Advisory)
            ),
        ),
        Verdict::Exceeded => (
            "exceeded",
            format!(
                "The profile exceeds the shaker's ratings: {}.",
                items(Verdict::Exceeded)
            ),
        ),
    }
}

/// `x` rounded to three significant digits.
fn sig3(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let e = x.abs().log10().floor() as i32;
    if e > 2 {
        let q = 10f64.powi(e - 2);
        (x / q).round() * q
    } else {
        let p = 10f64.powi(2 - e);
        (x * p).round() / p
    }
}

/// `x` written with three significant digits, trailing zeros kept: a measured
/// value beside a limit it passed by less than 1 % still reads as more than the
/// limit ("0.0312 against a 0.03 limit", not "0.03 against a 0.03 limit").
fn sig3_text(x: f64) -> String {
    let r = sig3(x);
    if r == 0.0 || !r.is_finite() {
        return format!("{:.2}", if r.is_finite() { 0.0 } else { r });
    }
    let e = r.abs().log10().floor() as i32;
    let d = (2 - e).max(0) as usize;
    format!("{r:.d$}")
}

/// The trip's facts for `end.trip`, and the end message.
fn trip_facts(t: &hertzforge_random::TripRecord, during_sysid: bool) -> (Value, String) {
    let at = super::config::round(t.at_level_db, 2);
    let at_words = |level_db: f64| -> String {
        if during_sysid {
            format!(
                "during system identification ({} against the reference)",
                db(level_db)
            )
        } else {
            format!("at {}", db(level_db))
        }
    };
    let (mut facts, message) = match &t.kind {
        TripKind::Envelope { abort_lines, worst_db } => (
            json!({"kind": "envelope", "abort_lines": abort_lines, "worst_db": num(super::config::round(finite_db(*worst_db), 2))}),
            format!(
                "Envelope abort: {abort_lines} lines outside the abort band, worst {}, {}.",
                db(finite_db(*worst_db)),
                at_words(t.at_level_db)
            ),
        ),
        TripKind::Kinematic { channel_index, axis, value, limit, .. } => {
            let axis = axis.to_ascii_lowercase();
            let axis = match axis.as_str() {
                "velocity" => "velocity",
                "displacement" => "displacement",
                _ => "accel",
            };
            (
                json!({"kind": "kinematic", "channel": channel_index, "axis": axis,
                       "value": num(sig3(*value)), "limit": num(*limit)}),
                format!(
                    "Kinematic abort on channel {channel_index}: {axis} {} against a {} limit, {}.",
                    sig3_text(*value),
                    num(*limit),
                    at_words(t.at_level_db)
                ),
            )
        }
        TripKind::DriveVrms { realized_vrms, limit_vrms } => (
            json!({"kind": "drive_vrms", "realized_vrms": num(super::config::round(*realized_vrms, 4)), "limit_vrms": num(*limit_vrms)}),
            format!(
                "Drive abort: {} V rms at or above the {} V limit, {}.",
                sig3_text(*realized_vrms),
                num(*limit_vrms),
                at_words(t.at_level_db)
            ),
        ),
        TripKind::Divergence { consecutive_loops, divergent_bins } => (
            json!({"kind": "divergence", "consecutive_loops": consecutive_loops, "divergent_bins": divergent_bins}),
            format!(
                "Divergence abort: {divergent_bins} lines diverged for {consecutive_loops} loops in a row, {}.",
                at_words(t.at_level_db)
            ),
        ),
        TripKind::LawFault { reason, consecutive } => (
            json!({"kind": "law_fault", "reason": reason.as_str(), "consecutive": consecutive}),
            format!(
                "The control law did not produce a usable drive for {consecutive} {} in a row \
                 ({}); the run stopped {}.",
                if *consecutive == 1 { "loop" } else { "loops" },
                reason.words(),
                at_words(t.at_level_db)
            ),
        ),
    };
    if let Value::Object(m) = &mut facts {
        m.insert("at_level_db".into(), num(at));
        m.insert("at_loop".into(), Value::from(t.at_loop));
    }
    (facts, message)
}

/// Each limited channel's ceiling on the band's lines of `loop_cfg`, on the lines
/// its rows cover: the controller's own construction (`ceiling_on_grid`).
pub fn limit_lines(loop_cfg: &hertzforge_random::LoopConfig, cfg: &WireConfig) -> Vec<LimitLines> {
    let (lo, hi) = loop_cfg.band_bins();
    let df = loop_cfg.df();
    cfg.limits
        .iter()
        .map(|l| {
            let ceiling = super::config::ceiling_on_grid(&l.rows, loop_cfg.nbins(), df);
            let (freqs, ceiling): (Vec<f64>, Vec<f64>) = (lo..hi.min(ceiling.len()))
                .filter(|k| ceiling[*k].is_finite())
                .map(|k| (k as f64 * df, ceiling[k]))
                .unzip();
            LimitLines {
                channel: l.channel,
                freqs,
                ceiling,
            }
        })
        .collect()
}

/// The enabled inputs, in table order.
pub fn live_channels(table: &ChannelTable) -> Vec<LiveChannel> {
    let mut inputs: Vec<&hertzforge_channel_table::Channel> = table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role != ChannelRole::Drive)
        .collect();
    inputs.sort_by_key(|c| table.channels.iter().position(|x| x.index == c.index));
    inputs
        .into_iter()
        .map(|c| LiveChannel {
            index: c.index,
            description: c.description.clone(),
            role: role_name(c.role),
            rms: None,
            unit: unit_name(&c.units),
            overload: false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_random::TripRecord;

    fn kinematic(value: f64, limit: f64) -> TripRecord {
        TripRecord {
            kind: TripKind::Kinematic {
                channel_index: 1,
                axis: "Accel".into(),
                value,
                limit,
                at_frame: 12,
            },
            at_level_db: -20.0,
            at_loop: 0,
        }
    }

    /// The bench's run 7: a 0.0312 g peak against a 0.03 g limit read "accel 0.03
    /// against a 0.03 limit", which reads as no exceedance (2026-11-01, fix
    /// verification observation 2). Three significant digits show it.
    #[test]
    fn a_law_fault_trip_says_why_and_that_the_run_stopped() {
        let t = TripRecord {
            kind: TripKind::LawFault {
                reason: hertzforge_random::LawFaultReason::OverBudget,
                consecutive: 3,
            },
            at_level_db: -6.0,
            at_loop: 14,
        };
        let (facts, message) = trip_facts(&t, false);
        assert_eq!(
            facts,
            json!({"kind": "law_fault", "reason": "over_budget", "consecutive": 3,
                   "at_level_db": -6, "at_loop": 14})
        );
        assert_eq!(
            message,
            "The control law did not produce a usable drive for 3 loops in a row (it took \
             longer than its time budget); the run stopped at -6.0 dB."
        );
    }

    #[test]
    fn a_kinematic_trip_shows_three_significant_digits() {
        let (facts, message) = trip_facts(&kinematic(0.031_24, 0.03), true);
        assert!(
            message.contains("accel 0.0312 against a 0.03 limit"),
            "{message}"
        );
        assert_eq!(facts["value"], json!(0.0312), "{facts}");
        let (facts, message) = trip_facts(&kinematic(0.411_49, 0.4), false);
        assert!(
            message.contains("accel 0.411 against a 0.4 limit"),
            "{message}"
        );
        assert_eq!(facts["value"], json!(0.411));
        let (_, message) = trip_facts(&kinematic(12.04, 12.0), false);
        assert!(
            message.contains("accel 12.0 against a 12 limit"),
            "{message}"
        );
        let (facts, message) = trip_facts(&kinematic(1234.5, 1000.0), false);
        assert!(
            message.contains("accel 1230 against a 1000 limit"),
            "{message}"
        );
        assert_eq!(facts["value"], json!(1230));
    }

    #[test]
    fn a_drive_trip_shows_three_significant_digits() {
        let t = TripRecord {
            kind: TripKind::DriveVrms {
                realized_vrms: 0.012_34,
                limit_vrms: 0.012,
            },
            at_level_db: -6.0,
            at_loop: 3,
        };
        let (_, message) = trip_facts(&t, false);
        assert!(
            message.contains("Drive abort: 0.0123 V rms at or above the 0.012 V limit"),
            "{message}"
        );
    }

    #[test]
    fn three_significant_digits_keep_their_trailing_zeros() {
        assert_eq!(sig3_text(0.03), "0.0300");
        assert_eq!(sig3_text(0.099_96), "0.100");
        assert_eq!(sig3_text(-2.5), "-2.50");
        assert_eq!(sig3_text(999.6), "1000");
        assert_eq!(sig3_text(0.0), "0.00");
        assert_eq!(sig3(0.031_24), 0.0312);
        assert_eq!(sig3(1234.5), 1230.0);
    }
}
