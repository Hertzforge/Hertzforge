//! One configured MIMO Sine run, on its own thread.
//!
//! The thread owns the engine's `MimoSineService` outright and drives it: arm,
//! identification, the coherence gate, the review hold, engage, then bounded
//! batches of ticks until the run ends. Nothing here is control math. The
//! socket never reaches the loop: requests arrive as [`Cmd`]s the thread takes
//! between batches, and the thread publishes an [`MsView`] after every step
//! that changes what it shows, and at the display's rate while the loop drives.
//!
//! The engine hands its update records and its timeline over channels that
//! never wait; this thread turns them into the view, the log and the run's
//! record. Every file is written by the record's own writer thread: this
//! thread only copies and sends.
//!
//! A stop or an abort must not wait for a long call. Identification is one: the
//! station is wrapped in [`Interruptible`], whose own stop takes the drive down
//! at the next engine block and ends the call. Once the loop drives, a stop is
//! the engine's 120 dB/s ramp on every drive at once, pumped to its end here;
//! an abort is the station's own stop at the next block. The engine's
//! protections (the band, limit, drive and kinematic aborts, the law's guards,
//! the divergence stop) stay the engine's.
//!
//! Time is the station's: every time the run reports is the engine's count of
//! ticks over the rate it adopted. On the simulator the thread paces the run
//! to the wall clock on that same count.

use super::config::{test_kind, WireMsine};
use super::laws::LawFacts;
use super::record::{self as mrec, Kept, TICK_BLOCK};
use super::view::{
    sig, Chan, End, Figures, IdentFigures, LawCounts, Live, MsView, Pass,
};
use crate::protocol::{num, ErrorCode, MethodError};
use crate::random::backend::{Interruptible, ABORT, NONE, STOP};
use crate::random::record as rrec;
use crate::random::run::{LevelTarget, LogEntry, NO_FRAME};
use crate::random::view::{RecordRef, State};
use crate::record::tap::{self, Recording};
use crate::record::{LogHandle, ProblemFn, RecordOptions, RecordStatus, RunRecord};
use crate::sine::config::round;
use crate::sine::view::StationRef;
use crate::station::{self, class_name, NiScanner, StationPlan};
use crate::table::unit_name;
use hertzforge_channel_table::ChannelTable;
use hertzforge_mimo_sine::core::Command;
use hertzforge_mimo_sine::records::{Event, EventKind, Outcome, TickRecord};
use hertzforge_mimo_sine::schedule::RequestOutcome;
use hertzforge_mimo_sine::service::{
    EngineOptions, IdentificationReport, MimoSineService, ServiceError, ServiceState,
};
use hertzforge_mimo_sine::{LevelMode, MimoSineLawFactory, Request};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// Who stops a run when the service shuts down.
pub const SHUTDOWN_BY: &str = crate::random::run::SHUTDOWN_BY;
/// The display's period: the run publishes its live numbers this often.
pub const PUBLISH_PERIOD: Duration = Duration::from_millis(200);
/// The identification's random seed.
pub const SYSID_SEED: u64 = 0x5EED_0003;
/// The engine's controlled stop on the station's clock, s: from full scale to
/// its floor at its rate.
pub const STOP_RAMP_S: f64 = STOP_RAMP_DB / hertzforge_mimo_sine::core::RAMP_DB_PER_S;

/// The engine's stop ramp's depth, dB: its floor scale, 1e-6.
const STOP_RAMP_DB: f64 = 120.0;

/// Engine batches run between two looks at the request queue.
const BATCHES: u32 = 8;

/// A publish, as the service hears it: the run id and the view published.
pub type Notice = (String, Arc<MsView>);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// What the run thread and the service share.
pub struct RunShared {
    pub view: Mutex<Arc<MsView>>,
    pub log: Mutex<Vec<LogEntry>>,
    /// The station signal: [`STOP`] or [`ABORT`] takes the drive down with the
    /// station's own stop at the next engine block.
    pub signal: Arc<AtomicU8>,
    /// A stop asked of the run during identification.
    pub stop_asked: AtomicBool,
    pub ended_by: Mutex<Option<String>>,
    pub notify: tokio::sync::mpsc::UnboundedSender<Notice>,
    /// The `mimo_sine.live` task has been started for this run.
    pub live_started: AtomicBool,
    pub record: Mutex<Option<LogHandle>>,
    pub station_frame: Arc<AtomicU64>,
}

impl RunShared {
    pub fn new(initial: MsView, notify: tokio::sync::mpsc::UnboundedSender<Notice>) -> RunShared {
        RunShared {
            view: Mutex::new(Arc::new(initial)),
            log: Mutex::new(Vec::new()),
            signal: Arc::new(AtomicU8::new(NONE)),
            stop_asked: AtomicBool::new(false),
            ended_by: Mutex::new(None),
            notify,
            live_started: AtomicBool::new(false),
            record: Mutex::new(None),
            station_frame: Arc::new(AtomicU64::new(NO_FRAME)),
        }
    }

    pub fn view(&self) -> Arc<MsView> {
        lock(&self.view).clone()
    }

    pub fn log_since(&self, since: u64) -> (Vec<LogEntry>, u64) {
        let log = lock(&self.log);
        let next = log.len() as u64;
        let from = (since as usize).min(log.len());
        (log[from..].to_vec(), next)
    }

    pub fn add_log(&self, kind: &'static str, text: impl Into<String>) {
        self.add_log_data(kind, text, None);
    }

    /// A log entry with structured facts for the record (`data`).
    pub fn add_log_data(&self, kind: &'static str, text: impl Into<String>, data: Option<Value>) {
        let mut log = lock(&self.log);
        let entry = LogEntry {
            seq: log.len() as u64,
            at_unix: now_unix(),
            kind,
            text: text.into(),
        };
        if let Some(r) = lock(&self.record).as_ref() {
            r.append(self.record_entry(&entry, data));
        }
        log.push(entry);
    }

    fn record_entry(&self, e: &LogEntry, data: Option<Value>) -> Value {
        let frame = self.station_frame.load(Ordering::Relaxed);
        let mut v = json!({"seq": e.seq, "at_unix": e.at_unix,
                           "frame": if frame == NO_FRAME { Value::Null } else { Value::from(frame) },
                           "kind": e.kind, "text": e.text});
        if let (Some(d), Value::Object(m)) = (data, &mut v) {
            m.insert("data".into(), d);
        }
        v
    }

    fn attach_record(&self, handle: LogHandle) {
        let log = lock(&self.log);
        for e in log.iter() {
            handle.append(self.record_entry(e, None));
        }
        *lock(&self.record) = Some(handle);
    }

    fn detach_record(&self) {
        let _log = lock(&self.log);
        *lock(&self.record) = None;
    }

    fn publish(&self, view: MsView) {
        let id = view.run_id.clone();
        let view = Arc::new(view);
        *lock(&self.view) = view.clone();
        let _ = self.notify.send((id, view));
    }

    /// Ask the run to stop or abort from the socket's thread, before the
    /// request is queued: identification ends at its next block, and an abort
    /// reaches the station at once.
    pub fn signal_end(&self, abort: bool, by: &str) {
        {
            let mut b = lock(&self.ended_by);
            if b.is_none() {
                *b = Some(by.to_owned());
            }
        }
        let in_sysid = {
            let v = self.view();
            v.state == State::Running && matches!(v.phase, "sysid" | "coherence_gate")
        };
        if abort {
            self.signal.store(ABORT, Ordering::SeqCst);
        } else if in_sysid {
            self.signal.store(STOP, Ordering::SeqCst);
        }
        if abort || in_sysid {
            self.stop_asked.store(true, Ordering::SeqCst);
        }
    }
}

/// What a run arms on, taken from the service when the request arrives.
pub struct ArmContext {
    pub table: ChannelTable,
    pub station: StationPlan,
    pub scanner: Arc<dyn NiScanner>,
    pub data_root: PathBuf,
    pub bind: String,
    /// The run's law, resolved from the catalog; `Err` is why it could not be.
    pub law: Result<Arc<dyn MimoSineLawFactory>, MethodError>,
    pub revision: u64,
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
        ack: Option<String>,
        reply: Reply<()>,
    },
    Level {
        target: LevelTarget,
        reply: Reply<f64>,
    },
    Mode {
        mode: LevelMode,
        reply: Reply<()>,
    },
    Hold {
        on: bool,
        reply: Reply<()>,
    },
    GoTo {
        hz: f64,
        relative: bool,
        reply: Reply<f64>,
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
    pub config: Arc<WireMsine>,
    pub law: LawFacts,
    /// Simulated seconds per wall second on the simulator; 0 runs unpaced.
    pub sim_speed: f64,
    pub capture_root: Option<PathBuf>,
    pub timeout_s: f64,
    pub record: RecordOptions,
    pub derived: Value,
    /// The simulator's structure the configuration's checks chose.
    pub structure: Option<String>,
}

/// What the record is kept with while the run goes.
struct Keeping {
    record: RunRecord,
    status: Arc<RecordStatus>,
    run_uid: String,
    kept: Kept,
    records: u64,
    /// Every control, limit channel and drive with its unit, in that order.
    channels: Vec<(usize, String)>,
    tick_s: f64,
}

/// The report's running figures.
#[derive(Default)]
struct Acc {
    /// Per control: worst |error| signed, its frequency, worst |phase error|,
    /// ticks in alarm, deepest notch dB.
    controls: Vec<(f64, f64, f64, u64, f64)>,
    /// Per limit channel: largest margin dB, its frequency, ticks notched.
    limits: Vec<(f64, f64, u64)>,
    /// Per drive: largest amplitude, its frequency, ticks over its ceiling.
    drives: Vec<(f64, f64, u64)>,
    opposition_max: f64,
    share_max: f64,
    held_ticks: u64,
}

/// The run thread's state.
struct Actor {
    plan: RunPlan,
    shared: Arc<RunShared>,
    svc: Option<MimoSineService>,
    lock: Option<hertzforge_station::lock::LockGuard>,
    view: MsView,
    table: ChannelTable,
    commands: Option<SyncSender<Command>>,
    records: Option<(Receiver<TickRecord>, Receiver<Event>)>,
    keep: Option<Keeping>,
    stop: Option<(&'static str, String)>,
    stop_at_s: Option<f64>,
    last_publish: Option<Instant>,
    pace_origin: Option<(Instant, f64)>,
    last: Option<TickRecord>,
    figures: Option<Figures>,
    pass_figures: Option<Figures>,
    pass: u64,
    last_dir: f64,
    level_since_s: f64,
    in_alarm: Vec<bool>,
    operator_hold: bool,
    acc: Acc,
    last_request: Option<(&'static str, bool)>,
    tick_s: f64,
    hardware: bool,
    /// The last samples the station read, for the `time` plot.
    time_ring: super::time::Shared,
}

fn refuse(reason: &'static str, message: impl Into<String>) -> MethodError {
    MethodError::named(ErrorCode::Refused, reason, message)
}

pub fn wrong_state(v: &MsView, verb: &str) -> MethodError {
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

pub fn not_a_sweep(test: &str, verb: &str) -> MethodError {
    refuse(
        "not_a_sweep",
        format!("{verb} applies to a sweep; this test is a {test}."),
    )
    .with_detail(json!({"test": test}))
}

fn arm_refused(words: impl std::fmt::Display) -> MethodError {
    refuse(
        "arm_refused",
        format!("The station refused to arm: {words}"),
    )
}

fn db(x: f64) -> String {
    format!("{} dB", num(round(x, 2)))
}

/// Start the run thread. Returns once the thread exists; the run is
/// `configured`.
pub fn spawn(
    plan: RunPlan,
    initial: MsView,
    shared: Arc<RunShared>,
    rx: Receiver<Cmd>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let name = format!("msine-{}", plan.run_id);
    std::thread::Builder::new().name(name).spawn(move || {
        let mut a = Actor {
            plan,
            shared,
            svc: None,
            lock: None,
            view: initial,
            table: ChannelTable::new(),
            commands: None,
            records: None,
            keep: None,
            stop: None,
            stop_at_s: None,
            last_publish: None,
            pace_origin: None,
            last: None,
            figures: None,
            pass_figures: None,
            pass: 0,
            last_dir: 0.0,
            level_since_s: 0.0,
            in_alarm: Vec::new(),
            operator_hold: false,
            acc: Acc::default(),
            last_request: None,
            tick_s: 0.0,
            hardware: false,
            time_ring: Default::default(),
        };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| a.run(rx)));
        if r.is_err() {
            // The run thread itself failed: the drive goes down with the
            // station's own stop, and the run says so.
            a.shared.signal.store(ABORT, Ordering::SeqCst);
            if let Some(svc) = a.svc.as_mut() {
                let _ = svc.run_batches(1);
            }
            a.svc = None;
            a.lock = None;
            let msg = "The run stopped on a fault: its thread failed; the drive was taken \
                       down with the station's own stop."
                .to_owned();
            a.view.end = Some(End {
                outcome: "fault",
                message: msg.clone(),
                at_s: a.view.elapsed_s,
                level_db: a.view.level_db,
                at_hz: a.view.frequency_hz,
                ramp_s: None,
                trip: None,
            });
            a.view.state = State::Finished;
            a.view.phase = "aborted";
            a.shared.add_log("note", msg);
            a.finish_record();
            a.shared.publish(a.view.clone());
        }
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
                        self.pump();
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
        if let Some(svc) = self.svc.as_mut() {
            self.shared.signal.store(ABORT, Ordering::SeqCst);
            let _ = svc.run_batches(1);
        }
        self.svc = None;
        self.lock = None;
        // The record keeps what it has, `complete: false`: the run did not end.
        self.shared.detach_record();
        self.keep = None;
    }

    fn publish(&mut self) {
        if let (Some(k), Some(r)) = (self.keep.as_ref(), self.view.record.as_mut()) {
            r.problem = k.status.problem();
        }
        if let Some(f) = &self.figures {
            self.view.figures = Some(Arc::new(f.clone()));
        }
        if self.view.streaming {
            let ring = match self.time_ring.lock() {
                Ok(g) => g.clone(),
                Err(p) => p.into_inner().clone(),
            };
            let inputs: Vec<(u32, Vec<f32>)> = rrec::input_rows(&self.table)
                .iter()
                .enumerate()
                .map(|(slot, c)| (c.index, ring.last(slot, super::time::RING_FRAMES)))
                .collect();
            self.view.time = Some(Arc::new(super::view::TimeFigures {
                fs_hz: self.view.grid.fs_hz,
                inputs,
            }));
        }
        self.last_publish = Some(Instant::now());
        self.shared.publish(self.view.clone());
    }

    fn set(&mut self, state: State, phase: &'static str) {
        if self.view.phase != phase || self.view.state != state {
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
            Cmd::Proceed { ack, reply } => {
                let r = self.proceed_cmd(ack);
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
            Cmd::Hold { on, reply } => {
                let r = self.hold_cmd(on);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::GoTo {
                hz,
                relative,
                reply,
            } => {
                let r = self.goto_cmd(hz, relative);
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
                    self.abort_now(&by);
                }
                self.publish();
                let _ = reply.send(Ok(()));
            }
        }
    }

    // ---- arm and start ---------------------------------------------------------

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
        self.view.streaming = true;
        self.set(State::Running, "sysid");
        let level = self.plan.config.setup.identification.level_db;
        self.shared.add_log_data(
            "note",
            format!("started; identification at {} under full level", db(level)),
            Some(json!({"action": "start", "level_db": level})),
        );
        if let Some(k) = self.keep.as_ref() {
            let frame = self.shared.station_frame.load(Ordering::Relaxed);
            let mut patch = Map::new();
            patch.insert(
                "start".into(),
                rrec::start_json(
                    now_unix(),
                    rrec::utc_offset_s(),
                    if frame == NO_FRAME { 0 } else { frame },
                ),
            );
            k.record.patch_manifest(patch);
        }
        Ok(())
    }

    /// Feasibility, the station, the arm gate and the record. Nothing is driven.
    fn arm(&mut self, ack: Option<String>, ctx: ArmContext) -> Result<(), MethodError> {
        let law = ctx.law?;
        let w = self.plan.config.clone();
        // Feasibility first: a blocked run opens nothing.
        let feas = w.feasibility_verdict();
        self.view.feasibility = feas.clone();
        if feas.0 == "exceeded" {
            match &ack {
                Some(a) => self.shared.add_log_data(
                    "override",
                    format!("operator acknowledgement: {a}"),
                    Some(json!({"what": "feasibility_exceeded", "acknowledgement": a})),
                ),
                None => {
                    self.set(State::Blocked, "feasibility_blocked");
                    return Err(refuse(
                        "feasibility_exceeded",
                        "Feasibility exceeded: a reference asks more of a shaker than its \
                         ratings. Review it, or arm with an acknowledgement.",
                    )
                    .with_detail(json!({"feasibility": {"verdict": feas.0, "message": feas.1}})));
                }
            }
        }
        let table = ctx.table.clone();
        let built = station::build(&ctx.station, &*ctx.scanner)?;
        let hardware = built.station.is_hardware();
        let lock = if hardware {
            Some(
                hertzforge_station::lock::acquire(
                    &ctx.data_root,
                    &ctx.bind,
                    &built.provenance.station_label,
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
        let class = built.station;
        let provenance = built.provenance.clone();
        let caps = built.backend.caps();
        let buffers = if w.record_streams {
            rrec::tap_buffers(w.fs_hz(), self.plan.record.stream_buffer_s)
        } else {
            1
        };
        let (inputs_tap, inputs_link) = tap::pair(
            rrec::input_rows(&table).len(),
            rrec::TAP_BLOCK_FRAMES,
            buffers,
        );
        let (drives_tap, drives_link) = tap::pair(
            rrec::drive_rows(&table).len(),
            rrec::TAP_BLOCK_FRAMES,
            buffers,
        );
        let recorded = Recording::wrap(
            built.backend,
            inputs_tap,
            drives_tap,
            self.shared.station_frame.clone(),
        );
        let timed = super::time::TimeTap::wrap(
            recorded,
            self.time_ring.clone(),
            rrec::input_rows(&table).len(),
        );
        let backend = Interruptible::wrap(timed, self.shared.signal.clone());
        let ratings = w.ratings();
        let mut opts = EngineOptions::new(class);
        opts.seed = SYSID_SEED;
        opts.ratings = ratings.clone();
        let mut svc = MimoSineService::new(w.setup.clone(), table.clone(), backend, law, opts)
            .map_err(|e| match e {
                ServiceError::Setup(s) => refuse(
                    s.rule.as_str(),
                    format!("The run could not be set up: {}", s.message),
                )
                .with_detail(json!({"field": s.field})),
                other => arm_refused(other),
            })?;
        svc.arm().map_err(arm_refused)?;
        let fs = svc.sample_rate_hz().unwrap_or(w.fs_hz());
        let frames = crate::sine::config::tick_frames(fs, w.tick_ms);
        self.view.grid = crate::sine::config::Grid {
            fs_hz: fs,
            tick_frames: frames,
            actual: true,
        };
        self.tick_s = frames as f64 / fs;

        // The run's record, before any sample: refused here, the arm is
        // refused and the station released (nothing has been driven).
        let armed_unix = now_unix();
        let utc_offset_s = rrec::utc_offset_s();
        let run_uid = rrec::run_uid(armed_unix, &self.plan.run_id);
        let root = self
            .plan
            .capture_root
            .clone()
            .unwrap_or_else(hertzforge_capture::default_data_root);
        let streams_off = if !w.record_streams {
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
        let f = &self.plan.law;
        let law_json = json!({
            "id": f.id, "name": f.name,
            "version": if f.version.is_empty() { Value::Null } else { Value::from(f.version.as_str()) },
            "sha256": if f.sha256.is_empty() { Value::Null } else { Value::from(f.sha256.as_str()) },
            "builtin": f.builtin,
            "params": super::law_params_json(f, &w.setup.law.params),
            "accepts": f.derived_json(),
            "values": f.values.iter().map(|d| json!({"key": d.key, "label": d.label, "unit": d.unit})).collect::<Vec<_>>(),
            "guards": {"law_budget": num(w.setup.guards.law_budget), "law_misses": w.setup.guards.law_misses,
                       "max_step_db": num(w.setup.guards.max_step_db),
                       "max_rise_db_per_s": num(w.setup.guards.max_rise_db_per_s),
                       "rotation_cap_deg": w.setup.guards.rotation_cap_deg.map(num),
                       "divergence_s": num(w.setup.guards.divergence_s)},
        });
        let facts = mrec::ArmFacts {
            run_id: &self.plan.run_id,
            run_uid: &run_uid,
            cfg: &w,
            table: &table,
            revision: ctx.revision,
            plan: &ctx.station,
            class,
            provenance: &provenance,
            devices: &ctx.devices,
            caps: &caps,
            fs_hz: fs,
            tick_frames: frames,
            derived: self.plan.derived.clone(),
            feasibility: &feas,
            law: law_json,
            ratings: &ratings,
            structure: self.plan.structure.as_deref(),
            armed_unix,
            utc_offset_s,
            streams_off: streams_off.clone(),
        };
        let begin = mrec::begin(&facts, root, (inputs_link, drives_link));
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
                    "The run's record could not be written, so the run did not arm: {} ({}). \
                     Choose a data folder that can be written.",
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
        for (name, d, arrays) in std::iter::once(mrec::reference_result(&w, &table))
            .chain(mrec::limit_results(&w, &table))
        {
            record.put_result(&name, d, arrays, None);
        }
        self.view.record = Some(RecordRef {
            folder: record.folder().display().to_string(),
            streams: streams_off.is_none(),
            problem: None,
        });
        let unit = |ch: usize| {
            table
                .channels
                .iter()
                .find(|c| c.index as usize == ch)
                .map_or_else(|| "g".to_owned(), |c| unit_name(&c.units).to_string())
        };
        let s = &w.setup;
        let channels: Vec<(usize, String)> = s
            .controls
            .iter()
            .map(|c| c.channel)
            .chain(s.limits.iter().map(|l| l.channel))
            .chain(s.drives.iter().map(|d| d.channel))
            .map(|c| (c, unit(c)))
            .collect();
        self.keep = Some(Keeping {
            status: record.status(),
            record,
            run_uid,
            kept: Kept::new(),
            records: 0,
            channels,
            tick_s: self.tick_s,
        });
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
        self.view.structure = if self.view.station.kind == "sim" {
            self.plan.structure.clone()
        } else {
            None
        };
        let describe = |ch: usize| -> Chan {
            let row = table.channels.iter().find(|c| c.index as usize == ch);
            Chan {
                channel: ch as u32,
                description: row.map_or_else(String::new, |c| c.description.clone()),
                unit: unit(ch),
            }
        };
        self.view.controls = s.controls.iter().map(|c| describe(c.channel)).collect();
        self.view.limits = s.limits.iter().map(|c| describe(c.channel)).collect();
        self.view.drives = s.drives.iter().map(|c| describe(c.channel)).collect();
        self.view.ceilings = s.drives.iter().map(|d| d.ceiling_vpk).collect();
        self.in_alarm = vec![false; s.controls.len()];
        self.acc.controls = vec![(0.0, f64::NAN, 0.0, 0, 0.0); s.controls.len()];
        self.acc.limits = vec![(f64::NEG_INFINITY, f64::NAN, 0); s.limits.len()];
        self.acc.drives = vec![(0.0, f64::NAN, 0); s.drives.len()];
        let against_time = matches!(s.test, hertzforge_mimo_sine::TestPlan::Dwell(_));
        let steps = w.steps();
        let mk = || {
            Figures::new(
                w.band(),
                steps.as_deref(),
                against_time,
                s.controls.len(),
                s.drives.len(),
                s.limits.len(),
            )
        };
        self.figures = Some(mk());
        self.pass_figures = Some(mk());
        self.table = table;
        self.svc = Some(svc);
        self.lock = lock;
        self.hardware = hardware;
        self.set(State::Armed, "armed");
        self.shared.add_log(
            "station",
            format!(
                "armed on {} at {} Hz, a {} frame tick, shared clock{}",
                class_name(class),
                num(round(fs, 3)),
                frames,
                match &self.view.structure {
                    Some(s) => format!(", structure {s}"),
                    None => String::new(),
                }
            ),
        );
        Ok(())
    }

    // ---- identification and the gate --------------------------------------------

    fn identify(&mut self) {
        let Some(svc) = self.svc.as_mut() else {
            self.fault("the run lost its station before identification");
            return;
        };
        let r = svc.identify().cloned();
        let signal = self.shared.signal.load(Ordering::SeqCst);
        if signal == ABORT {
            self.abort_now(&self.ender());
            return;
        }
        if self.shared.stop_asked.load(Ordering::SeqCst) || signal == STOP {
            let outcome = self.stop_outcome();
            self.end(outcome, "stopped", None, None, None);
            return;
        }
        let rep = match r {
            Ok(rep) => rep,
            Err(e) => {
                self.fault(&format!("identification failed: {e}"));
                return;
            }
        };
        self.set(State::Running, "coherence_gate");
        self.view.identification = Some(self.identification_json(&rep));
        self.view.review = Some(self.review_json(&rep, None));
        self.view.identification_figures = Some(Arc::new(ident_figures(
            &rep,
            &self.plan.config,
        )));
        if let Some(k) = self.keep.as_ref() {
            for (name, d, arrays) in
                mrec::identification_results(&rep, &self.plan.config, &self.table)
            {
                k.record.put_result(&name, d, arrays, None);
            }
            let mut patch = Map::new();
            patch.insert(
                "identification".into(),
                mrec::identification_json(&rep, None),
            );
            k.record.patch_manifest(patch);
        }
        let gate = self.plan.config.setup.identification.gate_coherence;
        let worst = rep
            .band_coherence
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        self.shared.add_log_data(
            "gate",
            format!(
                "identification: band coherence {} at worst against a gate of {}; {}",
                num(round(worst, 3)),
                num(gate),
                if rep.gate_passed { "passed" } else { "held" }
            ),
            Some(json!({"coherence_mean": rep.band_coherence, "gate_coherence": gate,
                        "passed": rep.gate_passed, "lag_frames": rep.lag_frames})),
        );
        for w in rep.sweep_rate_warning.iter().filter(|w| w.flagged) {
            self.shared.add_log_data(
                "note",
                format!(
                    "the sweep spends {} of its own time constants inside the {} Hz resonance's \
                     bandwidth; under two, the error there is expected to be large, and a \
                     slower sweep is the remedy",
                    num(round(w.time_constants, 2)),
                    num(round(w.f_hz, 1))
                ),
                Some(json!({"warning": "sweep_rate", "freq_hz": w.f_hz,
                            "time_constants": w.time_constants})),
            );
        }
        self.publish();
        let state = self.svc.as_ref().map(MimoSineService::state);
        match state {
            Some(ServiceState::CoherenceHeld) => {
                let words = format!(
                    "Coherence gate held: band coherence {} is below the gate of {}; the run \
                     never drove at test level.",
                    num(round(worst, 2)),
                    num(gate)
                );
                self.end("held", "coherence_hold", None, Some(words), None);
            }
            Some(ServiceState::AwaitingReview) => {
                self.set(State::Review, "awaiting_review");
                self.publish();
            }
            _ => {
                if let Err(e) = self.proceed() {
                    self.fault(&e.message);
                }
            }
        }
    }

    fn identification_json(&self, rep: &IdentificationReport) -> Value {
        let s = &self.plan.config.setup;
        let id = &rep.identification;
        let lines = id.frequency_hz.len().max(1);
        let band = self.plan.config.band();
        let nl = s.limits.len();
        let in_band: Vec<usize> = (0..id.frequency_hz.len())
            .filter(|&k| id.frequency_hz[k] >= band.0 && id.frequency_hz[k] <= band.1)
            .collect();
        let limit_mean: Vec<Value> = (0..nl)
            .map(|j| {
                let v: Vec<f64> = in_band
                    .iter()
                    .map(|&k| id.limit_coherence[j * lines + k])
                    .collect();
                num(round(v.iter().sum::<f64>() / v.len().max(1) as f64, 4))
            })
            .collect();
        let trusted = if in_band.is_empty() {
            0.0
        } else {
            in_band.iter().filter(|&&k| id.trusted[k]).count() as f64 / in_band.len() as f64
        };
        // The largest condition number of the response on a trusted line.
        let cond = hertzforge_sdm::linalg::condition_numbers(&id.response).unwrap_or_default();
        let cond_max = in_band
            .iter()
            .filter(|&&k| id.trusted[k])
            .filter_map(|&k| cond.get(k).copied())
            .filter(|c| c.is_finite())
            .fold(0.0f64, f64::max);
        json!({
            "level_db": num(s.identification.level_db),
            "measured_level_db": rep.level_db.map(|l| num(round(l, 2))),
            "realizations": rep.realizations,
            "line_spacing_hz": num(round(rep.line_spacing_hz, 6)),
            "duration_s": num(round(rep.duration_s, 2)),
            "drive_vrms": rep.drive_rms_v.iter().map(|v| num(sig(*v))).collect::<Vec<_>>(),
            "coherence_mean": rep.band_coherence.iter().map(|v| num(round(*v, 4))).collect::<Vec<_>>(),
            "limit_coherence_mean": limit_mean,
            "trusted_frac": num(round(trusted, 4)),
            "condition_max": num(round(cond_max, 2)),
            "lag_frames": rep.lag_frames,
            "gate_coherence": num(s.identification.gate_coherence),
            "passed": rep.gate_passed,
        })
    }

    fn review_json(&self, rep: &IdentificationReport, ack: Option<&str>) -> Value {
        json!({
            "resonances": rep.sweep_rate_warning.iter().map(|w| json!({
                "freq_hz": num(round(w.f_hz, 2)),
                "bandwidth_hz": num(round(w.f_hz / w.q.max(1e-9), 3)),
                "time_constants": num(round(w.time_constants, 3)),
                "flagged": w.flagged,
            })).collect::<Vec<_>>(),
            "acknowledged": ack,
        })
    }

    fn ender(&self) -> String {
        lock(&self.shared.ended_by)
            .clone()
            .unwrap_or_else(|| "the operator".into())
    }

    fn stop_outcome(&self) -> &'static str {
        self.stop.as_ref().map_or("operator_stop", |(o, _)| *o)
    }

    fn fault(&mut self, words: &str) {
        self.shared.signal.store(ABORT, Ordering::SeqCst);
        if let Some(svc) = self.svc.as_mut() {
            let _ = svc.run_batches(1);
        }
        self.end(
            "fault",
            "aborted",
            None,
            Some(format!("The run stopped on a fault: {words}")),
            None,
        );
    }

    // ---- review and engage ------------------------------------------------------

    fn proceed_cmd(&mut self, ack: Option<String>) -> Result<(), MethodError> {
        if self.view.state != State::Review {
            return Err(wrong_state(&self.view, "Proceeding"));
        }
        self.shared.add_log_data(
            "note",
            match &ack {
                Some(a) => format!("the operator reviewed the identification and proceeded: {a}"),
                None => "the operator reviewed the identification and proceeded".into(),
            },
            Some(json!({"action": "proceed", "acknowledgement": ack})),
        );
        if let (Some(svc), Some(a)) = (self.svc.as_ref(), ack.as_deref()) {
            if let Some(rep) = svc.identification() {
                let rep = rep.clone();
                self.view.review = Some(self.review_json(&rep, Some(a)));
            }
        }
        if let Some(svc) = self.svc.as_mut() {
            svc.proceed_after_review()
                .map_err(|e| refuse("wrong_state", e.to_string()))?;
        }
        self.proceed()
    }

    /// Build the law and engage at the start level.
    fn proceed(&mut self) -> Result<(), MethodError> {
        let svc = self
            .svc
            .as_mut()
            .ok_or_else(|| wrong_state(&self.view, "Proceeding"))?;
        let r = svc.engage();
        let state = svc.state();
        match r {
            Ok(()) => {}
            Err(e) if state == ServiceState::Ended(Outcome::LawFault) => {
                let trip = json!({"kind": "law_fault", "at_hz": null, "at_level_db": null,
                                  "at_tick": null, "reason": "error", "consecutive": 0});
                self.shared
                    .add_log_data("trip", e.to_string(), Some(trip.clone()));
                self.end(
                    "law_fault",
                    "stopped",
                    Some(trip),
                    Some(format!(
                        "The control law could not be built, so the tone never started: {e}"
                    )),
                    None,
                );
                return Ok(());
            }
            Err(e) => {
                self.fault(&format!("the run could not engage: {e}"));
                return Ok(());
            }
        }
        self.commands = Some(svc.commands());
        self.records = svc.take_records();
        if let (Some(k), Some(rep)) = (self.keep.as_ref(), svc.identification()) {
            let mut patch = Map::new();
            patch.insert(
                "identification".into(),
                mrec::identification_json(rep, svc.law_setup()),
            );
            k.record.patch_manifest(patch);
        }
        self.view.law_built = true;
        self.view.level_db = Some(self.plan.config.setup.schedule.ladder.start_db);
        self.set(State::Running, "engaged");
        self.publish();
        Ok(())
    }

    // ---- the loop -----------------------------------------------------------------

    fn pump(&mut self) {
        if self.shared.signal.load(Ordering::SeqCst) == ABORT {
            self.abort_now(&self.ender());
            return;
        }
        let Some(svc) = self.svc.as_mut() else {
            self.fault("the run lost its station");
            return;
        };
        let r = svc.run_batches(BATCHES);
        self.drain();
        match r {
            Ok(true) => {
                self.pace();
                let due = self
                    .last_publish
                    .is_none_or(|t| t.elapsed() >= PUBLISH_PERIOD);
                let changed = super::view::state_key(Some(&self.shared.view()))
                    != super::view::state_key(Some(&self.view));
                if due || changed {
                    self.publish();
                }
            }
            Ok(false) => self.finish_from_engine(),
            Err(e) => {
                if self.shared.signal.load(Ordering::SeqCst) == ABORT {
                    self.abort_now(&self.ender());
                } else {
                    self.fault(&format!("the loop failed: {e}"));
                }
            }
        }
    }

    /// Sleep while the station's time runs ahead of the wall clock on the
    /// simulator.
    fn pace(&mut self) {
        if self.plan.sim_speed <= 0.0 || self.view.station.kind != "sim" {
            return;
        }
        let Some(t) = self.view.elapsed_s else { return };
        let (w0, t0) = *self.pace_origin.get_or_insert((Instant::now(), t));
        let ahead = (t - t0) / self.plan.sim_speed - w0.elapsed().as_secs_f64();
        if ahead > 0.002 {
            std::thread::sleep(Duration::from_secs_f64(ahead.min(0.25)));
        }
    }

    /// Take every update record and event the engine has handed over.
    fn drain(&mut self) {
        let Some((ticks, events)) = self.records.take() else {
            return;
        };
        let evs: Vec<Event> = events.try_iter().collect();
        for e in evs {
            self.on_event(&e);
        }
        let recs: Vec<TickRecord> = ticks.try_iter().collect();
        for r in recs {
            self.on_tick(&r);
        }
        self.records = Some((ticks, events));
    }

    fn channel_of_control(&self, c: usize) -> u32 {
        self.view.controls.get(c).map_or(0, |x| x.channel)
    }

    fn on_event(&mut self, e: &Event) {
        let at = json!({"t_s": num(round(e.time_s, 4)), "tick": e.index,
                        "frequency_hz": num(round(e.frequency_hz, 4)),
                        "level_db": num(round(e.level_db, 3))});
        let with = |extra: Value| -> Value {
            let mut v = at.clone();
            if let (Value::Object(m), Value::Object(x)) = (&mut v, extra) {
                m.extend(x);
            }
            v
        };
        let hz = num(round(e.frequency_hz, 2));
        match e.kind {
            EventKind::Engaged => self.shared.add_log_data(
                "level",
                format!("engaged at {} and {hz} Hz", db(e.level_db)),
                Some(with(json!({"event": "engaged"}))),
            ),
            EventKind::Level => {
                self.level_since_s = e.time_s;
                self.shared.add_log_data(
                    "level",
                    format!("level {} at {hz} Hz", db(e.level_db)),
                    Some(with(json!({"event": "level"}))),
                )
            }
            EventKind::Held { by_law } => self.shared.add_log_data(
                "hold",
                if by_law {
                    format!("the schedule held at the law's request at {hz} Hz")
                } else {
                    format!("the schedule held at {hz} Hz")
                },
                Some(with(json!({"event": "held", "by_law": by_law}))),
            ),
            EventKind::Resumed => self.shared.add_log_data(
                "hold",
                format!("the schedule resumed at {hz} Hz"),
                Some(with(json!({"event": "resumed"}))),
            ),
            EventKind::ManualFrequency => self.shared.add_log_data(
                "note",
                format!("the operator moved the tone to {hz} Hz"),
                Some(with(json!({"event": "manual_frequency"}))),
            ),
            EventKind::NotchIn { depth } => self.shared.add_log_data(
                "notch",
                format!(
                    "the limit channels hold the reference down to {} of it at {hz} Hz",
                    num(round(depth, 4))
                ),
                Some(with(json!({"event": "notch_in", "depth": num(round(depth, 5))}))),
            ),
            EventKind::NotchOut => self.shared.add_log_data(
                "notch",
                format!("the limit channels allow the whole reference again at {hz} Hz"),
                Some(with(json!({"event": "notch_out"}))),
            ),
            EventKind::NotchedReferenceRefused => self.shared.add_log_data(
                "notch",
                format!("the law's notched reference was not taken at {hz} Hz"),
                Some(with(json!({"event": "notched_reference_refused"}))),
            ),
            EventKind::Alarm { control, error_db } => {
                if let Some(a) = self.in_alarm.get_mut(control as usize) {
                    *a = true;
                }
                let ch = self.channel_of_control(control as usize);
                self.shared.add_log_data(
                    "alarm",
                    format!(
                        "control {ch} left its alarm band at {hz} Hz ({} dB)",
                        num(round(error_db, 2))
                    ),
                    Some(with(json!({"event": "alarm", "channel": ch,
                                     "error_db": num(round(error_db, 3))}))),
                );
            }
            EventKind::AlarmCleared { control } => {
                if let Some(a) = self.in_alarm.get_mut(control as usize) {
                    *a = false;
                }
                let ch = self.channel_of_control(control as usize);
                self.shared.add_log_data(
                    "alarm",
                    format!("control {ch} came back inside its alarm band at {hz} Hz"),
                    Some(with(json!({"event": "alarm_cleared", "channel": ch}))),
                );
            }
            EventKind::LawMiss { reason } => self.shared.add_log_data(
                "law",
                format!("the law's call was not used ({}) at {hz} Hz", reason.replace('_', " ")),
                Some(with(json!({"event": "law_miss", "reason": reason}))),
            ),
            EventKind::OverCeiling { scale } => self.shared.add_log_data(
                "alarm",
                format!(
                    "a drive over its ceiling scaled the drive vector by {} at {hz} Hz",
                    num(round(scale, 4))
                ),
                Some(with(json!({"event": "over_ceiling", "scale": num(round(scale, 5))}))),
            ),
            EventKind::Stopping { outcome } => {
                if self.stop_at_s.is_none() {
                    self.stop_at_s = Some(e.time_s);
                }
                if outcome == Outcome::Completed {
                    self.shared.add_log_data(
                        "note",
                        format!("the schedule is complete at {hz} Hz; every drive ramps down"),
                        Some(with(json!({"event": "stopping", "outcome": outcome.as_str()}))),
                    );
                } else if self.view.end.is_none() && !matches!(outcome, Outcome::Stopped) {
                    self.shared.add_log_data(
                        "trip",
                        format!(
                            "{} at {hz} Hz; the drive ramps down",
                            outcome.as_str().replace('_', " ")
                        ),
                        Some(with(json!({"event": "stopping", "outcome": outcome.as_str()}))),
                    );
                }
                if self.view.state == State::Running {
                    self.set(State::Stopping, "ramping_down");
                }
            }
            EventKind::Ended { outcome } => self.shared.add_log_data(
                "state",
                format!("every drive at zero ({})", outcome.as_str().replace('_', " ")),
                Some(with(json!({"event": "ended", "outcome": outcome.as_str()}))),
            ),
        }
    }

    fn on_tick(&mut self, r: &TickRecord) {
        let (n, m, p) = (r.controls as usize, r.drives as usize, r.limits as usize);
        let wrap = |d: f64| {
            let x = (d + 180.0).rem_euclid(360.0) - 180.0;
            if x <= -180.0 {
                x + 360.0
            } else {
                x
            }
        };
        let level = 10f64.powf(r.level_db / 20.0);
        let mut live = Live {
            t_s: r.time_s,
            controls: Vec::with_capacity(n),
            limits: Vec::with_capacity(p),
            drives: Vec::with_capacity(m),
            opposition_ratio: r.opposition_ratio,
            largest_share: r.largest_share,
            rotation_deg: r.largest_rotation_deg,
            request: None,
            values: r.values[..self.view.law_values.len().min(r.values.len())].to_vec(),
            law_faulted: r.law_faulted,
            sweep_hz_per_s: r.sweep_hz_per_s,
            sweep_oct_per_min: r.sweep_oct_per_min,
            commanded_db: r.level_db,
        };
        for c in 0..n {
            let amp = r.control[c].norm();
            let ph = wrap(r.control[c].arg().to_degrees());
            let judged = r.judged_reference[c].norm();
            let refa = r.reference[c].norm();
            let rph = wrap(r.reference[c].arg().to_degrees());
            let depth = if judged > 0.0 && refa > 0.0 {
                (20.0 * (refa / judged).log10()).max(0.0)
            } else {
                0.0
            };
            live.controls
                .push([amp, ph, judged, rph, r.error_db[c], wrap(ph - rph), depth]);
            if !r.settling {
                let a = &mut self.acc.controls[c];
                if r.error_db[c].is_finite() && r.error_db[c].abs() > a.0.abs() {
                    a.0 = r.error_db[c];
                    a.1 = r.frequency_hz;
                }
                let pe = wrap(ph - rph).abs();
                if self.view.phase_specified && pe > a.2 {
                    a.2 = pe;
                }
                a.4 = a.4.max(depth);
            }
            if self.in_alarm.get(c).copied().unwrap_or(false) {
                self.acc.controls[c].3 += 1;
            }
        }
        for j in 0..p {
            let amp = r.limit[j].norm();
            let b = r.limit_bound[j];
            let margin = 20.0 * (amp / b).log10();
            live.limits.push([amp, wrap(r.limit[j].arg().to_degrees()), b, margin]);
            let a = &mut self.acc.limits[j];
            if margin.is_finite() && margin > a.0 {
                a.0 = margin;
                a.1 = r.frequency_hz;
            }
            if r.notch_scale < 1.0 {
                a.2 += 1;
            }
        }
        let mut worst_over = (0usize, 0.0f64);
        for d in 0..m {
            let amp = r.drive[d].norm();
            live.drives.push([amp, wrap(r.drive[d].arg().to_degrees())]);
            let a = &mut self.acc.drives[d];
            if amp > a.0 {
                a.0 = amp;
                a.1 = r.frequency_hz;
            }
            let ratio = amp / self.view.ceilings.get(d).copied().unwrap_or(f64::INFINITY);
            if ratio > worst_over.1 {
                worst_over = (d, ratio);
            }
        }
        if r.ceiling_scale < 1.0 {
            if let Some(a) = self.acc.drives.get_mut(worst_over.0) {
                a.2 += 1;
            }
            self.view.counts.over_ceiling += 1;
        }
        if r.opposition_ratio.is_finite() {
            self.acc.opposition_max = self.acc.opposition_max.max(r.opposition_ratio);
        }
        self.acc.share_max = self.acc.share_max.max(r.largest_share);
        // The law's request.
        let granted = matches!(
            r.request_outcome,
            RequestOutcome::Granted | RequestOutcome::Bounded
        );
        if let Some(q) = r.request {
            let (kind, factor) = match q {
                Request::Hold => ("hold", None),
                Request::Slow(f) => ("slow", Some(f)),
            };
            live.request = Some((kind, factor, granted));
            self.view.counts.asked += 1;
            if granted {
                self.view.counts.granted += 1;
                if kind == "hold" {
                    self.acc.held_ticks += 1;
                }
            }
            if self.last_request != Some((kind, granted)) {
                self.shared.add_log_data(
                    "request",
                    format!(
                        "the law asks to {} the sweep at {} Hz; {}",
                        kind,
                        num(round(r.frequency_hz, 2)),
                        r.request_outcome.as_str().replace('_', " ")
                    ),
                    Some(json!({"tick": r.index, "t_s": num(round(r.time_s, 4)),
                                "frequency_hz": num(round(r.frequency_hz, 4)),
                                "request": kind, "factor": factor.map(num),
                                "outcome": r.request_outcome.as_str()})),
                );
            }
            self.last_request = Some((kind, granted));
        } else {
            self.last_request = None;
        }
        self.view.request_held = granted;
        // The law's counts.
        if r.law_called {
            if r.miss.is_some() {
                self.view.counts.misses += 1;
            } else {
                self.view.counts.misses = 0;
            }
        }
        if r.cap_bound || r.rotation_bound {
            self.view.counts.capped_ticks += 1;
        }
        self.view.counts.rotation_max_deg =
            self.view.counts.rotation_max_deg.max(r.largest_rotation_deg);
        // The schedule's place.
        let prev_freq = self.last.as_ref().map(|l| l.frequency_hz);
        let dir = r.sweep_hz_per_s.signum() * f64::from(u8::from(r.sweep_hz_per_s != 0.0));
        let sweep = self.plan.config.is_sweep();
        if sweep && dir != 0.0 {
            if self.last_dir != 0.0 && dir != self.last_dir {
                self.end_pass();
            }
            self.last_dir = dir;
            self.view.direction = Some(if dir > 0.0 { "up" } else { "down" });
        }
        if let (Some(f0), false) = (prev_freq, sweep) {
            if (r.frequency_hz - f0).abs() > 1e-9 * f0.abs().max(1.0) {
                if let Some(s) = self.view.steps.as_mut() {
                    s.0 = (s.0 + 1).min(s.1);
                }
            }
        }
        if let Some(l) = &self.last {
            if (l.level_db - r.level_db).abs() > 1e-9 {
                self.level_since_s = r.time_s;
            }
        }
        self.view.frequency_hz = Some(r.frequency_hz);
        self.view.level_db = Some(r.level_db);
        self.view.rung = self
            .plan
            .config
            .setup
            .schedule
            .ladder
            .table_db
            .iter()
            .position(|x| (x - r.level_db).abs() < 1e-6);
        self.view.elapsed_s = Some(r.time_s);
        self.view.at_level_s = Some(r.time_s - self.level_since_s);
        self.view.remaining_s = self
            .plan
            .derived
            .get("test_s")
            .and_then(Value::as_f64)
            .map(|t| (t - r.time_s).max(0.0));
        self.view.notch = r.notch_scale < 1.0;
        self.view.notch_scale = r.notch_scale;
        self.view.alarm = self.in_alarm.iter().any(|a| *a);
        self.view.held = self.operator_hold;
        if let Some(s) = self.view.sweeps.as_mut() {
            s.0 = self.pass;
        }
        let _ = level;
        // Figures.
        if let Some(f) = self.figures.as_mut() {
            f.put(r.frequency_hz, &live, r.settling);
        }
        if let Some(f) = self.pass_figures.as_mut() {
            f.put(r.frequency_hz, &live, r.settling);
        }
        self.view.live = Some(Arc::new(live));
        // The record.
        let pass = self.pass as f64;
        if let Some(k) = self.keep.as_mut() {
            k.kept.ticks.push(*r);
            k.kept.pass.push(pass);
            if k.kept.ticks.len() >= TICK_BLOCK {
                Self::put_block(k, &self.view.law_values);
            }
        }
        self.last = Some(*r);
    }

    /// A one-way sweep's pass has ended: its figures become the sweep frame.
    fn end_pass(&mut self) {
        self.pass += 1;
        let Some(f) = self.pass_figures.as_mut() else {
            return;
        };
        let fresh = Figures::new(
            self.plan.config.band(),
            self.plan.config.steps().as_deref(),
            f.against_time,
            f.control.len(),
            f.drive.len(),
            f.limit.len(),
        );
        let done = std::mem::replace(f, fresh);
        let reached = done
            .control
            .first()
            .is_some_and(|c| c.iter().any(|p| p[0].is_finite()));
        if reached && !done.against_time {
            self.view.last_pass = Some(Arc::new(Pass {
                pass: self.pass,
                level_db: self.view.level_db.unwrap_or(0.0),
                direction: if self.plan.config.is_sweep() {
                    self.view.direction
                } else {
                    None
                },
                figures: done,
            }));
        }
    }

    fn put_block(k: &mut Keeping, values: &[(String, String, String)]) {
        if k.kept.ticks.is_empty() {
            return;
        }
        let nc = k.kept.ticks[0].controls as usize;
        let nl = k.kept.ticks[0].limits as usize;
        let drives: Vec<usize> = k.channels[nc + nl..].iter().map(|(c, _)| *c).collect();
        let first = k.kept.ticks.first().map(|r| r.index);
        let last = k.kept.ticks.last().map(|r| r.index);
        let (name, d, arrays) = mrec::levels_result(&k.kept, &k.channels, nc, nl, k.tick_s);
        let line = json!({"index": k.records, "points": k.kept.ticks.len(),
                          "first_tick": first, "last_tick": last});
        k.record.put_result(&name, d, arrays, Some(line.clone()));
        let keys: Vec<String> = values.iter().map(|(k, _, _)| k.clone()).collect();
        let (name, d, arrays) = mrec::trace_result(&k.kept, &drives, &keys);
        k.record.put_result(&name, d, arrays, Some(line));
        k.records += 1;
        k.kept = Kept::new();
    }

    // ---- operator verbs ------------------------------------------------------------

    fn driving(&self) -> bool {
        self.view.state == State::Running
            && matches!(self.view.phase, "engaged" | "full_level")
            && self.commands.is_some()
    }

    fn send(&self, c: Command) -> Result<(), MethodError> {
        self.commands
            .as_ref()
            .ok_or_else(|| wrong_state(&self.view, "This"))?
            .try_send(c)
            .map_err(|_| MethodError::internal("the run's loop did not take the request"))
    }

    fn level_cmd(&mut self, target: LevelTarget) -> Result<f64, MethodError> {
        if !self.driving() {
            return Err(wrong_state(&self.view, "A level change"));
        }
        if self.view.mode != LevelMode::Manual {
            return Err(refuse(
                "manual_mode_required",
                "The level moves by hand only in manual mode; switch the ladder to manual \
                 first.",
            ));
        }
        let levels = &self.plan.config.setup.schedule.ladder.table_db;
        let now = self.view.level_db.unwrap_or(0.0);
        let delta = match target {
            LevelTarget::Delta(d) => d,
            LevelTarget::To(t) => {
                if !levels.iter().any(|l| (l - t).abs() < 1e-9) {
                    return Err(refuse(
                        "not_a_rung",
                        format!("{} is not a level of the schedule.", db(t)),
                    )
                    .with_detail(json!({"levels_db": levels})));
                }
                t - now
            }
        };
        let lo = levels.first().copied().unwrap_or(0.0);
        let hi = levels.last().copied().unwrap_or(0.0);
        let to = (now + delta).clamp(lo, hi);
        self.send(Command::StepLevel(to - now))?;
        self.shared.add_log_data(
            "level",
            format!("the operator moved the level to {}", db(to)),
            Some(json!({"action": "level", "level_db": num(to)})),
        );
        Ok(to)
    }

    fn mode_cmd(&mut self, mode: LevelMode) -> Result<(), MethodError> {
        if self.view.state.is_live() {
            return Err(refuse(
                "mode_fixed",
                "The ladder's mode is fixed once the run has armed; it is set in the \
                 configuration (schedule.mode).",
            ));
        }
        if self.view.state == State::Finished {
            return Err(self.finished_refusal());
        }
        if self.view.mode != mode {
            self.view.mode = mode;
            let mut w = (*self.plan.config).clone();
            w.setup.schedule.mode = mode;
            self.plan.config = Arc::new(w);
            self.shared.add_log_data(
                "note",
                format!("ladder mode {}", super::config::mode_name(mode)),
                Some(json!({"action": "mode", "mode": super::config::mode_name(mode)})),
            );
        }
        Ok(())
    }

    fn hold_cmd(&mut self, on: bool) -> Result<(), MethodError> {
        if !self.driving() {
            return Err(wrong_state(
                &self.view,
                if on { "Holding" } else { "Resuming" },
            ));
        }
        self.send(if on { Command::Hold } else { Command::Resume })?;
        self.operator_hold = on;
        self.view.held = on;
        Ok(())
    }

    fn goto_cmd(&mut self, hz: f64, relative: bool) -> Result<f64, MethodError> {
        if !self.driving() {
            return Err(wrong_state(&self.view, "Moving the frequency"));
        }
        let band = self.plan.config.band();
        let to = if relative {
            (self.view.frequency_hz.unwrap_or(band.0) + hz).clamp(band.0, band.1)
        } else {
            hz
        };
        self.send(Command::GoTo(to))?;
        Ok(to)
    }

    fn stop_cmd(&mut self, outcome: &'static str, by: String) {
        if !self.view.state.is_live() || self.view.state == State::Stopping {
            return;
        }
        self.stop = Some((outcome, by.clone()));
        self.shared.add_log_data(
            "note",
            if outcome == "safe_stop" && by == SHUTDOWN_BY {
                "the control service is shutting down; stopping the run with its controlled stop"
                    .to_owned()
            } else if outcome == "safe_stop" {
                format!(
                    "nobody held control for {} s; stopping the run with its controlled stop",
                    num(self.plan.timeout_s)
                )
            } else {
                format!("stop requested by {by}")
            },
            Some(json!({"action": "stop", "outcome": outcome, "by": by})),
        );
        match self.view.state {
            State::Armed | State::Review => self.end(outcome, "stopped", None, None, None),
            State::Running if self.driving() => {
                if self.send(Command::Stop).is_err() {
                    self.fault("the run's loop did not take the stop");
                } else {
                    self.stop_at_s = self.view.elapsed_s;
                    self.set(State::Stopping, "ramping_down");
                }
            }
            // Identification ends on the station's stop, raised before this
            // request was queued.
            _ => {}
        }
    }

    fn abort_now(&mut self, by: &str) {
        if self.view.state.is_live() {
            self.shared.add_log_data(
                "note",
                format!("abort requested by {by}"),
                Some(json!({"action": "abort", "by": by})),
            );
        }
        self.shared.signal.store(ABORT, Ordering::SeqCst);
        if let Some(svc) = self.svc.as_mut() {
            // The next engine block reads the signal and takes every drive down
            // with the station's own stop.
            let _ = svc.run_batches(1);
        }
        self.drain();
        {
            let mut b = lock(&self.shared.ended_by);
            if b.is_none() {
                *b = Some(by.to_owned());
            }
        }
        self.end("operator_abort", "aborted", None, None, None);
    }

    /// The engine ended the run on this batch.
    fn finish_from_engine(&mut self) {
        let Some(svc) = self.svc.as_ref() else { return };
        let state = svc.state();
        let note = svc.station_note().map(str::to_owned);
        // The engine's stop ramp takes every drive from its value to the floor
        // at a fixed rate on the station's clock: its length is the floor in dB
        // over the rate, whatever the drive was.
        let ramp = Some(STOP_RAMP_S);
        let outcome = match state {
            ServiceState::Ended(o) => o,
            other => {
                self.fault(&format!("the loop ended in state {other:?}"));
                return;
            }
        };
        if self.shared.signal.load(Ordering::SeqCst) == ABORT {
            self.abort_now(&self.ender());
            return;
        }
        match outcome {
            Outcome::Completed => {
                if let Some(s) = self.view.steps.as_mut() {
                    s.0 = s.1;
                }
                let words = match self.plan.config.test_s() {
                    Some(s) if self.view.mode == LevelMode::Auto => format!(
                        "Test complete: the schedule ran its {} s of test time and every drive \
                         ramped down with the controlled stop.",
                        num(round(s, 1))
                    ),
                    _ => "Test complete: the schedule finished and every drive ramped down \
                          with the controlled stop."
                        .to_owned(),
                };
                self.end("completed", "stopped", None, Some(words), ramp)
            }
            Outcome::Stopped => {
                let outcome = self.stop_outcome();
                self.end(outcome, "stopped", None, None, ramp)
            }
            Outcome::RunThreadFailure => self.fault("the loop's thread failed; the station was stopped at once"),
            Outcome::Station => self.fault(&format!(
                "the station stopped the run ({})",
                note.unwrap_or_else(|| "no reason given".into())
            )),
            o => {
                let (trip, message) = self.trip_facts(o);
                self.shared
                    .add_log_data("trip", message.clone(), Some(trip.clone()));
                let (name, phase) = if o == Outcome::LawFault {
                    ("law_fault", "stopped")
                } else {
                    ("aborted", "aborted")
                };
                self.end(name, phase, Some(trip), Some(message), ramp);
            }
        }
    }

    /// A trip's facts and words, from the last update before the stop.
    fn trip_facts(&self, o: Outcome) -> (Value, String) {
        let r = self.last.as_ref();
        let at_hz = r.map(|r| r.frequency_hz);
        let at_level = r.map(|r| r.level_db);
        let tick = r.map(|r| r.index);
        let s = &self.plan.config.setup;
        let mut t = json!({"at_hz": at_hz.map(|f| num(round(f, 3))),
                           "at_level_db": at_level.map(|l| num(round(l, 2))),
                           "at_tick": tick});
        let hz = num(round(at_hz.unwrap_or(0.0), 2));
        let words;
        match o {
            Outcome::ControlAbort { control } => {
                let c = control as usize;
                let ch = self.channel_of_control(c);
                let (measured, err) = r.map_or((f64::NAN, f64::NAN), |r| {
                    (r.control[c].norm(), r.error_db[c])
                });
                let side = if err >= 0.0 { "high" } else { "low" };
                let f = at_hz.unwrap_or(0.0);
                let lvl = 10f64.powf(at_level.unwrap_or(0.0) / 20.0);
                let limit = s.controls.get(c).and_then(|cc| cc.profile.abort_bounds(f)).map(
                    |b| if side == "high" { b.1 * lvl } else { b.0 * lvl },
                );
                t["kind"] = json!("abort_band");
                t["channel"] = json!(ch);
                t["side"] = json!(side);
                t["measured"] = num(sig(measured));
                t["limit"] = limit.map_or(Value::Null, |l| num(sig(l)));
                words = format!(
                    "Aborted: control {ch} passed its {side} abort band at {hz} Hz ({} dB).",
                    num(round(err, 2))
                );
            }
            Outcome::LimitAbort { limit } => {
                let j = limit as usize;
                let ch = self.view.limits.get(j).map_or(0, |l| l.channel);
                let measured = r.map_or(f64::NAN, |r| r.limit[j].norm());
                let f = at_hz.unwrap_or(0.0);
                let bound = s.limits.get(j).and_then(|l| l.profile.abort_bounds(f)).map(|b| b.1);
                t["kind"] = json!("limit_channel");
                t["channel"] = json!(ch);
                t["measured"] = num(sig(measured));
                t["limit"] = bound.map_or(Value::Null, |l| num(sig(l)));
                words = format!("Aborted: limit channel {ch} passed its abort bound at {hz} Hz.");
            }
            Outcome::DriveAbort { drive } => {
                let d = drive as usize;
                let ch = self.view.drives.get(d).map_or(0, |x| x.channel);
                let vpk = r.map_or(f64::NAN, |r| r.drive[d].norm());
                t["kind"] = json!("drive_vpk");
                t["channel"] = json!(ch);
                t["vpk"] = num(sig(vpk));
                t["abort_vpk"] = num(s.drives.get(d).map_or(f64::NAN, |x| x.abort_vpk));
                words = format!("Aborted: drive {ch} reached its abort at {hz} Hz.");
            }
            Outcome::Kinematic { control } => {
                let c = control as usize;
                let drive = self.view.drives.get(c).map(|d| d.channel);
                t["kind"] = json!("kinematic");
                t["drive"] = json!(drive);
                t["control"] = json!(self.channel_of_control(c));
                t["axis"] = Value::Null;
                t["value"] = Value::Null;
                t["limit"] = Value::Null;
                words = format!(
                    "Aborted: the motion at control {} passed its shaker's rating at {hz} Hz.",
                    self.channel_of_control(c)
                );
            }
            Outcome::LawFault => {
                let reason = r.and_then(|r| r.miss).unwrap_or("error");
                t["kind"] = json!("law_fault");
                t["reason"] = json!(reason);
                t["consecutive"] = json!(self.view.counts.misses);
                words = format!(
                    "The law gave no usable drive {} times in a row ({}); the run took its \
                     controlled stop at {hz} Hz.",
                    self.view.counts.misses,
                    reason.replace('_', " ")
                );
            }
            Outcome::Divergence => {
                let cap = if r.is_some_and(|r| r.rotation_bound) {
                    "rotation"
                } else {
                    "step"
                };
                t["kind"] = json!("divergence");
                t["capped_s"] = num(s.guards.divergence_s);
                t["cap"] = json!(cap);
                words = format!(
                    "Aborted: a cap bound for {} s without a break at {hz} Hz; the run took its \
                     controlled stop.",
                    num(s.guards.divergence_s)
                );
            }
            Outcome::FrequencyOutOfRange => {
                t["kind"] = json!("oscillator_range");
                t["requested_hz"] = at_hz.map_or(Value::Null, num);
                t["fs_hz"] = num(self.view.grid.fs_hz);
                words = "Aborted: the tone left what the station can play.".into();
            }
            _ => {
                t["kind"] = json!(o.as_str());
                words = format!("The run ended: {}.", o.as_str().replace('_', " "));
            }
        }
        (t, words)
    }

    /// End the run, release the station, and say how it ended.
    fn end(
        &mut self,
        outcome: &'static str,
        phase: &'static str,
        trip: Option<Value>,
        message: Option<String>,
        ramp_s: Option<f64>,
    ) {
        if self.view.state == State::Finished {
            return;
        }
        self.drain();
        let by = self
            .stop
            .as_ref()
            .map(|(_, b)| b.clone())
            .unwrap_or_else(|| self.ender());
        let driven = self.last.is_some();
        let (level, at_hz) = match &trip {
            Some(t) => (
                t.get("at_level_db").and_then(Value::as_f64),
                t.get("at_hz").and_then(Value::as_f64),
            ),
            None if driven => (self.view.level_db, self.view.frequency_hz),
            None => (None, None),
        };
        let at = match level {
            Some(l) => format!(
                "{} and {} Hz after {} s",
                db(l),
                num(round(at_hz.unwrap_or(0.0), 2)),
                num(round(self.view.elapsed_s.unwrap_or(0.0), 2))
            ),
            None => "before driving".to_owned(),
        };
        let message = message.unwrap_or_else(|| match outcome {
            "operator_stop" => format!("Stopped by {by} at {at}."),
            "operator_abort" => format!("Aborted by {by} at {at}."),
            "safe_stop" if by == SHUTDOWN_BY => format!(
                "The control service was shut down, so it stopped the run with its controlled \
                 stop, from {at}."
            ),
            "safe_stop" => format!(
                "Nobody held control for {} s, so the service stopped the run with its \
                 controlled stop, from {at}.",
                num(self.plan.timeout_s)
            ),
            _ => "The run ended.".to_owned(),
        });
        let unramped = matches!(outcome, "aborted" | "operator_abort" | "fault") && ramp_s.is_none();
        if matches!(outcome, "operator_abort" | "fault") {
            self.shared.signal.store(ABORT, Ordering::SeqCst);
        }
        // The station is released before the end is published.
        if let Some(svc) = self.svc.as_mut() {
            if !matches!(svc.state(), ServiceState::Ended(_)) && driven {
                self.shared.signal.store(ABORT, Ordering::SeqCst);
                let _ = svc.run_batches(1);
            }
        }
        self.svc = None;
        self.lock = None;
        if self.plan.config.is_sweep() || !self.pass_figures.as_ref().is_some_and(|f| f.against_time)
        {
            self.end_pass();
        }
        // Every drive is at zero now: the ramp, or the station's own stop.
        if let Some(l) = self.view.live.as_deref() {
            let mut l = l.clone();
            l.drives.iter_mut().for_each(|d| d[0] = 0.0);
            self.view.live = Some(Arc::new(l));
        }
        self.view.end = Some(End {
            outcome,
            message: message.clone(),
            at_s: self.view.elapsed_s,
            level_db: level,
            at_hz,
            ramp_s: if unramped { None } else { ramp_s },
            trip,
        });
        self.set(State::Finished, phase);
        self.view.held = false;
        self.view.request_held = false;
        self.shared
            .add_log_data("note", message, Some(json!({"outcome": outcome, "by": by})));
        self.view.report = Some(self.report_json());
        self.finish_record();
        self.publish();
    }

    fn report_json(&self) -> Value {
        let s = &self.plan.config.setup;
        let tick = self.tick_s;
        json!({
            "run_id": self.view.run_id,
            "title": self.view.title,
            "source": self.view.station.source(),
            "grid": self.view.grid.to_json(),
            "test": self.view.test,
            "shape": self.view.shape,
            "phase_specified": self.view.phase_specified,
            "law": self.view.law,
            "levels_db": self.view.levels_db.iter().map(|l| num(*l)).collect::<Vec<_>>(),
            "controls": self.view.controls.iter().zip(&self.acc.controls).map(|(c, a)| json!({
                "channel": c.channel,
                "worst_error_db": num(round(a.0, 3)),
                "worst_error_hz": num(round(a.1, 3)),
                "worst_phase_error_deg": if self.view.phase_specified { num(round(a.2, 2)) } else { Value::Null },
                "alarm_s": num(round(a.3 as f64 * tick, 3)),
                "notch_depth_max_db": num(round(a.4, 3)),
            })).collect::<Vec<_>>(),
            "limits": self.view.limits.iter().zip(&self.acc.limits).map(|(c, a)| json!({
                "channel": c.channel,
                "worst_margin_db": num(round(a.0, 3)),
                "worst_margin_hz": num(round(a.1, 3)),
                "notched_s": num(round(a.2 as f64 * tick, 3)),
            })).collect::<Vec<_>>(),
            "drives": self.view.drives.iter().zip(&self.acc.drives).enumerate().map(|(i, (c, a))| json!({
                "channel": c.channel,
                "vpk_max": num(sig(a.0)),
                "vpk_max_hz": num(round(a.1, 3)),
                "ceiling_vpk": num(s.drives.get(i).map_or(f64::NAN, |d| d.ceiling_vpk)),
                "over_ceiling_ticks": a.2,
            })).collect::<Vec<_>>(),
            "readings": {"opposition_ratio_max": num(round(self.acc.opposition_max, 4)),
                         "largest_share_max": num(round(self.acc.share_max, 4))},
            "requests": {"asked": self.view.counts.asked, "granted": self.view.counts.granted,
                         "held_s": num(round(self.acc.held_ticks as f64 * tick, 3))},
            "identification": self.view.identification.clone().unwrap_or(Value::Null),
            "end": self.view.end.as_ref().map_or(Value::Null, End::to_json),
        })
    }

    /// End the record: the part-filled block, `end.json` with the run's end as
    /// its state reports it and the report, then `complete`.
    fn finish_record(&mut self) {
        let Some(mut k) = self.keep.take() else {
            return;
        };
        Self::put_block(&mut k, &self.view.law_values);
        self.shared.detach_record();
        let frame = self.shared.station_frame.load(Ordering::Relaxed);
        let mut end = Map::new();
        end.insert("run_uid".into(), Value::from(k.run_uid.as_str()));
        end.insert(
            "end".into(),
            self.view.end.as_ref().map_or(Value::Null, End::to_json),
        );
        end.insert("at_unix".into(), Value::from(now_unix()));
        end.insert(
            "station_frames".into(),
            Value::from(if frame == NO_FRAME { 0 } else { frame }),
        );
        end.insert(
            "summary".into(),
            json!({"elapsed_s": self.view.elapsed_s.map(|t| num(round(t, 4))),
                   "records": k.records,
                   "dropped": self.svc.as_ref().map_or(0, |s| s.records_dropped())}),
        );
        end.insert(
            "report".into(),
            self.view.report.clone().unwrap_or(Value::Null),
        );
        if let Some(r) = self.view.record.as_mut() {
            r.problem = k.status.problem();
        }
        k.record.finish(end);
    }
}

/// The identification as drawn: its grid inside the band (a dwell's an octave
/// either side), every row's gain and phase from every drive, and coherence.
fn ident_figures(rep: &IdentificationReport, w: &WireMsine) -> IdentFigures {
    let id = &rep.identification;
    let s = &w.setup;
    let (nc, nl, m) = (s.controls.len(), s.limits.len(), s.drives.len());
    let lines = id.frequency_hz.len();
    let b = w.band();
    let (lo, hi) = if (b.1 - b.0).abs() < 1e-6 {
        (b.0 * 0.5, b.0 * 2.0)
    } else {
        b
    };
    let idx: Vec<usize> = (0..lines)
        .filter(|&k| id.frequency_hz[k] >= lo && id.frequency_hz[k] <= hi)
        .collect();
    let h = |r: usize, d: usize, k: usize| {
        if r < nc {
            id.response.get(k, r, d)
        } else {
            id.limit_response.get(k, r - nc, d)
        }
    };
    let coh = |r: usize, k: usize| {
        if r < nc {
            id.coherence[r * lines + k]
        } else {
            id.limit_coherence[(r - nc) * lines + k]
        }
    };
    IdentFigures {
        freqs_hz: idx.iter().map(|&k| id.frequency_hz[k]).collect(),
        gain: (0..nc + nl)
            .map(|r| (0..m).map(|d| idx.iter().map(|&k| h(r, d, k).norm()).collect()).collect())
            .collect(),
        phase: (0..nc + nl)
            .map(|r| {
                (0..m)
                    .map(|d| idx.iter().map(|&k| h(r, d, k).arg().to_degrees()).collect())
                    .collect()
            })
            .collect(),
        coherence: (0..nc + nl)
            .map(|r| idx.iter().map(|&k| coh(r, k)).collect())
            .collect(),
        trust_coherence: s.identification.trust_coherence,
    }
}

/// A run's view as configured: nothing open, the law named.
pub fn configured_view(
    run_id: &str,
    w: &WireMsine,
    derived: &super::config::Derived,
    station: StationRef,
    law: Value,
    law_values: Vec<(String, String, String)>,
) -> MsView {
    let s = &w.setup;
    MsView {
        run_id: run_id.to_owned(),
        title: w.title.clone(),
        state: State::Configured,
        phase: "idle",
        structure: if station.kind == "sim" {
            derived.structure.clone()
        } else {
            None
        },
        station,
        grid: derived.grid,
        clock: None,
        test: test_kind(&s.test),
        shape: derived.shape,
        phase_specified: s.phase_specified,
        mode: s.schedule.mode,
        levels_db: s.schedule.ladder.table_db.clone(),
        level_db: None,
        rung: None,
        frequency_hz: None,
        direction: None,
        sweeps: if w.is_sweep() {
            Some((
                0,
                match s.test {
                    hertzforge_mimo_sine::TestPlan::Sweep(c) => match c.target {
                        hertzforge_sine::SweepTarget::Sweeps(n) => Some(u64::from(n)),
                        hertzforge_sine::SweepTarget::WhicheverFirst { sweeps, .. } => {
                            Some(u64::from(sweeps))
                        }
                        hertzforge_sine::SweepTarget::DurationS(_) => None,
                    },
                    _ => None,
                },
            ))
        } else {
            None
        },
        steps: w.steps().map(|v| (0, v.len() as u64)),
        held: false,
        request_held: false,
        elapsed_s: None,
        at_level_s: None,
        remaining_s: None,
        alarm: false,
        notch: false,
        notch_scale: 1.0,
        identification: None,
        review: None,
        law,
        law_values,
        law_built: false,
        counts: LawCounts::default(),
        feasibility: derived.feasibility.clone(),
        end: None,
        record: None,
        streaming: false,
        controls: Vec::new(),
        limits: Vec::new(),
        drives: Vec::new(),
        ceilings: s.drives.iter().map(|d| d.ceiling_vpk).collect(),
        live: None,
        figures: None,
        last_pass: None,
        identification_figures: None,
        report: None,
        time: None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn mimo_sine_the_stop_ramp_length_is_the_engines() {
        let db = -20.0 * hertzforge_mimo_sine::core::RAMP_FLOOR_SCALE.log10();
        assert!((db - super::STOP_RAMP_DB).abs() < 1e-9);
        assert!((super::STOP_RAMP_S - 1.0).abs() < 1e-9);
    }
}
