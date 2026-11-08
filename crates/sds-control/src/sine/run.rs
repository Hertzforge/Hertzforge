//! One configured Sine run, on its own thread.
//!
//! The thread owns the controller's `SineService` outright and drives it the way
//! the controller's own host does: feasibility, arm, identification, the coherence
//! gate, the review hold, then `run_chunk` a quarter second of station time at a
//! time until the run ends. Nothing here is control math. The socket never reaches the loop: requests
//! arrive as [`Cmd`]s the thread takes between batches, and the thread publishes a
//! [`SineView`] after every step that changes what it shows, and at the display's
//! rate while the loop drives.
//!
//! A stop or an abort must not wait for a long call. Identification is one: the
//! controller's own cancel flag ends it (raised by the service for a stop or an
//! abort, never read back as one), and an abort also reaches the station directly
//! through [`Interruptible`], whose own stop takes the drive down at the next
//! engine block. Once the loop drives, a stop is the controller's 120 dB/s ramp,
//! pumped to its end here; the station's own stop is never used for it. The
//! controller's protections (the band aborts, the kinematic, drive, limit and
//! servo-fault aborts, the watchdog, the law's guards) stay the controller's.
//!
//! Time is the station's: every time the run reports (`t_s`, `elapsed_s`, the
//! end's `at_s` and `ramp_s`) is the controller's own count of frames over the
//! rate it adopted. On the simulator the thread paces the run to the wall clock
//! on that same count.

use super::config::{LawFacts, WireSine, WireTest};
use super::view::{
    Alarm, End, Figures, IdentFacts, IdentFigures, InputCurves, LiveChannel, Rate, SineView, State,
    StationRef, IDENT_POINTS,
};
use crate::protocol::{num, ErrorCode, MethodError};
use crate::random::backend::{Interruptible, ABORT, NONE};
use crate::random::run::{LevelTarget, LogEntry};
use crate::station::{self, class_name, NiScanner, StationPlan};
use crate::table::{role_name, unit_name};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_sine::service::report::EventKind;
use hertzforge_sine::service::AdjustRefusal;
use hertzforge_sine::{
    RunMode, RunState, ServiceError, SineDisplay, SineLawFactory, SineService, SineStatus,
    SweepRate, TripKind, TripRecord,
};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// Who stops a run when the service shuts down. Its safe stop says so.
pub const SHUTDOWN_BY: &str = crate::random::run::SHUTDOWN_BY;
/// The display's period: the run publishes its live numbers this often.
pub const PUBLISH_PERIOD: Duration = Duration::from_millis(200);
/// The identification's random seed.
const SYSID_SEED: u64 = 0x5EED_0001;

/// A publish, as the service hears it: the run id and the view published.
pub type Notice = (String, Arc<SineView>);

/// What the run thread and the service share.
pub struct RunShared {
    pub view: Mutex<Arc<SineView>>,
    pub log: Mutex<Vec<LogEntry>>,
    /// The station signal: [`ABORT`] takes the drive down with the station's own
    /// stop at the next engine block.
    pub signal: Arc<AtomicU8>,
    /// A stop asked of the run, from the socket's thread: identification ends at
    /// its next block on the controller's own cancel flag.
    pub stop_asked: AtomicBool,
    /// The controller's cancel flag for identification, once the run has armed.
    pub sysid_cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// Who sent the stop or abort.
    pub ended_by: Mutex<Option<String>>,
    pub notify: tokio::sync::mpsc::UnboundedSender<Notice>,
    /// The `sine.live` task has been started for this run.
    pub live_started: AtomicBool,
}

impl RunShared {
    pub fn new(initial: SineView, notify: tokio::sync::mpsc::UnboundedSender<Notice>) -> RunShared {
        RunShared {
            view: Mutex::new(Arc::new(initial)),
            log: Mutex::new(Vec::new()),
            signal: Arc::new(AtomicU8::new(NONE)),
            stop_asked: AtomicBool::new(false),
            sysid_cancel: Mutex::new(None),
            ended_by: Mutex::new(None),
            notify,
            live_started: AtomicBool::new(false),
        }
    }

    pub fn view(&self) -> Arc<SineView> {
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
        let mut log = match self.log.lock() {
            Ok(l) => l,
            Err(p) => p.into_inner(),
        };
        let seq = log.len() as u64;
        log.push(LogEntry {
            seq,
            at_unix: now_unix(),
            kind,
            text: text.into(),
        });
    }

    fn publish(&self, view: SineView) {
        let id = view.run_id.clone();
        let view = Arc::new(view);
        match self.view.lock() {
            Ok(mut v) => *v = view.clone(),
            Err(p) => *p.into_inner() = view.clone(),
        }
        let _ = self.notify.send((id, view));
    }

    /// Ask the run to stop or abort from the socket's thread, before the request
    /// is queued: identification ends at its next block, and an abort reaches the
    /// station at once.
    pub fn signal_end(&self, abort: bool, by: &str) {
        if let Ok(mut b) = self.ended_by.lock() {
            if b.is_none() {
                *b = Some(by.to_owned());
            }
        }
        if abort {
            self.signal.store(ABORT, Ordering::SeqCst);
        }
        let in_sysid = {
            let v = self.view();
            v.state == State::Running && matches!(v.phase, "sysid" | "coherence_gate")
        };
        if abort || in_sysid {
            self.stop_asked.store(true, Ordering::SeqCst);
            if let Some(f) = self.sysid_cancel.lock().ok().and_then(|c| c.clone()) {
                f.store(true, Ordering::SeqCst);
            }
        }
    }
}

fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// What a run arms on, taken from the service when the request arrives.
pub struct ArmContext {
    pub table: ChannelTable,
    pub station: StationPlan,
    pub scanner: Arc<dyn NiScanner>,
    pub data_root: PathBuf,
    pub bind: String,
    /// The run's law, resolved from the catalog; `Err` is why it could not be.
    pub law: Result<Arc<dyn SineLawFactory>, MethodError>,
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
    Hold {
        on: bool,
        reply: Reply<()>,
    },
    Reverse {
        reply: Reply<()>,
    },
    Jog {
        delta_hz: f64,
        reply: Reply<f64>,
    },
    GoTo {
        hz: f64,
        reply: Reply<f64>,
    },
    Adjust {
        key: String,
        value: f64,
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
    pub config: Arc<WireSine>,
    /// The run's law as its configuration's checks saw it.
    pub law: LawFacts,
    /// Simulated seconds per wall second on the simulator; 0 runs unpaced (tests).
    pub sim_speed: f64,
    /// The capture root handed to the controller (`None`: its local default).
    pub capture_root: Option<PathBuf>,
    /// The orphan timeout, for the safe stop's message.
    pub timeout_s: f64,
}

/// The run thread's state.
struct Actor {
    plan: RunPlan,
    shared: Arc<RunShared>,
    svc: Option<SineService>,
    lock: Option<hertzforge_station::lock::LockGuard>,
    view: SineView,
    table: ChannelTable,
    /// The controller's log events already forwarded.
    forwarded: usize,
    /// How a requested stop ends the run, and who asked; `None`: no stop asked.
    stop: Option<(&'static str, String)>,
    /// Station time when the controlled stop was asked.
    stop_at_t: Option<f64>,
    /// Station time before the last batch: a completion's ramp runs inside it.
    t_before: Option<f64>,
    /// The scheduled level and frequency when the end was first asked.
    end_level: Option<(Option<f64>, Option<f64>)>,
    /// The rung and the sweep count at its entry, for `sweeps.done`.
    rung_base: (usize, u32),
    last_publish: Option<Instant>,
    /// Wall time and station time of the first batch, for the simulator's pacing.
    pace_origin: Option<(Instant, f64)>,
    /// The last batch's samples of each enabled input, and their rate.
    time: Vec<(u32, Vec<f32>)>,
    time_fs_hz: f64,
    identification: Option<IdentFigures>,
    alarm_was: bool,
}

fn refuse(reason: &'static str, message: impl Into<String>) -> MethodError {
    MethodError::named(ErrorCode::Refused, reason, message)
}

pub fn wrong_state(v: &SineView, verb: &str) -> MethodError {
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

pub fn adjust_refused(key: &str, why: &'static str, message: impl Into<String>) -> MethodError {
    refuse("adjust_refused", message).with_detail(json!({"key": key, "why": why}))
}

fn arm_refused(words: impl std::fmt::Display) -> MethodError {
    refuse(
        "arm_refused",
        format!("The station refused to arm: {words}"),
    )
}

fn db(x: f64) -> String {
    format!("{} dB", num(super::config::round(x, 2)))
}

/// Start the run thread. Returns once the thread exists; the run is `configured`.
pub fn spawn(
    plan: RunPlan,
    initial: SineView,
    shared: Arc<RunShared>,
    rx: Receiver<Cmd>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let name = format!("sine-{}", plan.run_id);
    std::thread::Builder::new().name(name).spawn(move || {
        let mut a = Actor {
            plan,
            shared,
            svc: None,
            lock: None,
            view: initial,
            table: ChannelTable::new(),
            forwarded: 0,
            stop: None,
            stop_at_t: None,
            t_before: None,
            end_level: None,
            rung_base: (0, 0),
            last_publish: None,
            pace_origin: None,
            time: Vec::new(),
            time_fs_hz: 0.0,
            identification: None,
            alarm_was: false,
        };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| a.run(rx)));
        if r.is_err() {
            // The run thread itself failed: the drive goes down with the station's
            // own stop, and the run says so.
            a.shared.signal.store(ABORT, Ordering::SeqCst);
            if let Some(svc) = a.svc.as_mut() {
                let _ = svc.abort("the run thread failed");
            }
            a.svc = None;
            a.lock = None;
            let msg = "The run stopped on a fault: its thread failed; the drive was taken \
                       down with the station's own stop."
                .to_owned();
            a.view.end = Some(End {
                outcome: "fault",
                message: msg.clone(),
                at_s: a.view.t_s,
                level_db: a.view.level_db,
                at_hz: a.view.frequency_hz,
                ramp_s: None,
                trip: None,
            });
            a.view.state = State::Finished;
            a.view.phase = "aborted";
            a.shared.add_log("note", msg);
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
            let _ = svc.abort("the service released the run");
        }
        self.svc = None;
        self.lock = None;
    }

    fn publish(&mut self) {
        self.last_publish = Some(Instant::now());
        self.shared.publish(self.view.clone());
    }

    fn set(&mut self, state: State, phase: &'static str) {
        if self.view.phase != phase {
            self.shared.add_log(
                "state",
                format!(
                    "{} to {}",
                    self.view.phase.replace('_', " "),
                    phase.replace('_', " ")
                ),
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
            Cmd::Hold { on, reply } => {
                let r = self.hold_cmd(on);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Reverse { reply } => {
                let r = self.sweep_cmd("Reversing the sweep", |s| s.reverse().map(|_| 0.0));
                self.publish();
                let _ = reply.send(r.map(|_| ()));
            }
            Cmd::Jog { delta_hz, reply } => {
                let r = self.sweep_cmd("A jog", |s| s.jog(delta_hz));
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::GoTo { hz, reply } => {
                let r = self.goto_cmd(hz);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Adjust { key, value, reply } => {
                let r = self.adjust_cmd(&key, value);
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

    // ---- arm and start -------------------------------------------------------------

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
        self.shared.add_log(
            "note",
            format!(
                "started; identification at {} under full level",
                db(self.plan.config.ident_level_db)
            ),
        );
        Ok(())
    }

    /// The controller's configuration for the station's table.
    fn controller_config(&self, table: &ChannelTable) -> hertzforge_sine::SineTestConfig {
        self.plan.config.build(table, &self.plan.law)
    }

    /// Feasibility, the station and the arm gate. Nothing is driven.
    fn arm(&mut self, ack: Option<String>, ctx: ArmContext) -> Result<(), MethodError> {
        let law = ctx.law?;
        let cfg = self.controller_config(&ctx.table);
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
        let backend = Interruptible::wrap(station.backend, self.shared.signal.clone());
        let svc = SineService::new(
            cfg,
            table.clone(),
            backend,
            class,
            SYSID_SEED,
            self.plan.capture_root.clone(),
        )
        .map_err(arm_refused)?;
        // The simulator's own block is one frame; the run pumps the controller a
        // batch at a time, so the simulator takes the block a hardware station
        // would (the rate's), and a batch is the same length of station time on both.
        let svc = if hardware {
            svc
        } else {
            svc.with_block_frames(hertzforge_hal::default_block_frames(
                self.plan.config.fs_hz(),
            ))
        };
        let mut svc = svc.with_law_factory(law);
        svc.set_run_mode(self.view.mode)
            .map_err(|e| arm_refused(e.to_string()))?;
        if let Ok(mut c) = self.shared.sysid_cancel.lock() {
            *c = Some(svc.sysid_cancel_flag());
        }
        let feas = svc.assess_feasibility().map_err(arm_refused)?;
        let (verdict, words) = crate::shock::config::verdict(&feas);
        self.view.feasibility = Some((
            verdict,
            if self.plan.config.feasibility.is_some() {
                words.replace("The pulse", "The profile")
            } else {
                "No shaker was given, so the profile was not checked against shaker ratings.".into()
            },
        ));
        self.forward_log(&svc);
        if svc.state() == RunState::FeasibilityBlocked {
            match &ack {
                Some(a) => {
                    svc.arm_with_override(a).map_err(arm_refused)?;
                    self.shared
                        .add_log("override", format!("operator acknowledgement: {a}"));
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
        let fs = svc.sample_rate_hz();
        let frames = svc
            .config()
            .control_tick_s
            .map(|t| super::config::tick_frames(fs, t * 1000.0))
            .unwrap_or(svc.config().control_tick_frames);
        self.view.grid = super::config::Grid {
            fs_hz: fs,
            tick_frames: frames,
            actual: true,
        };
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
        self.view.channels = live_channels(&table);
        self.view.control_unit = control_unit(&table);
        self.time_fs_hz = fs;
        self.table = table;
        self.svc = Some(svc);
        self.lock = lock;
        self.set(State::Armed, "armed");
        self.shared.add_log(
            "station",
            format!(
                "armed on {} at {} Hz, a {} frame tick, shared clock",
                class_name(class),
                num(super::config::round(fs, 3)),
                frames
            ),
        );
        Ok(())
    }

    // ---- identification and the gate --------------------------------------------------

    fn identify(&mut self) {
        let Some(svc) = self.svc.as_mut() else {
            self.fault("the run lost its station before identification");
            return;
        };
        let r = svc.run_sysid(0);
        self.forward_log_now();
        if self.shared.signal.load(Ordering::SeqCst) == ABORT {
            self.abort_now(&self.ender());
            return;
        }
        if self.shared.stop_asked.load(Ordering::SeqCst) {
            self.end_stopped(self.stop_outcome(), None);
            return;
        }
        if let Err(e) = r {
            if self.svc.as_ref().map(SineService::state) == Some(RunState::Aborted) {
                self.finish_from_controller();
            } else {
                self.fault(&format!("identification failed: {e}"));
            }
            return;
        }
        self.identification = self.identification_figures();
        self.set(State::Running, "coherence_gate");
        self.publish();
        let Some(svc) = self.svc.as_mut() else { return };
        let gate = svc.coherence_gate();
        let state = svc.state();
        self.forward_log_now();
        self.ident_facts();
        match gate {
            Err(e) => self.fault(&format!("the coherence gate failed: {e}")),
            Ok(_) if state == RunState::CoherenceHold => {
                let c = self
                    .view
                    .identification
                    .as_ref()
                    .map_or(0.0, |i| i.band_coherence);
                let floor = self.plan.config.gate_coherence;
                self.end(
                    "held",
                    "coherence_hold",
                    None,
                    Some(format!(
                        "Coherence gate held: band coherence {c:.2} is below the gate of \
                         {floor:.2}; the run never drove at test level."
                    )),
                    None,
                );
            }
            Ok(_) if state == RunState::AwaitingReview => {
                if self.plan.config.review {
                    self.set(State::Review, "awaiting_review");
                    self.publish();
                } else if let Err(e) = self.proceed() {
                    self.fault(&e.message);
                }
            }
            Ok(_) => self.engaged(),
        }
    }

    /// The gate's facts from the controller's status.
    fn ident_facts(&mut self) {
        let Some(svc) = self.svc.as_ref() else { return };
        let st = svc.live_status();
        self.view.identification = st.identification.map(|i| IdentFacts {
            level_db: i.level_db,
            band_coherence: i.band_coherence,
            gate_coherence: i.gate_coherence,
            lag_frames: i.lag_frames,
            trusted_frac: i.trusted_fraction,
            passed: i.passed,
        });
    }

    /// The identification as drawn: its grid inside the band (a dwell's an octave
    /// either side, as the gate judges it), at most [`IDENT_POINTS`] points.
    fn identification_figures(&self) -> Option<IdentFigures> {
        let svc = self.svc.as_ref()?;
        let frf = svc.frf()?;
        let (lo, hi) = {
            let b = self.plan.config.test.band();
            if (b.1 - b.0).abs() < 1e-6 {
                (b.0 * 0.5, b.0 * 2.0)
            } else {
                b
            }
        };
        let idx: Vec<usize> = frf
            .freqs
            .iter()
            .enumerate()
            .filter(|(_, f)| **f >= lo && **f <= hi)
            .map(|(i, _)| i)
            .collect();
        if idx.is_empty() {
            return None;
        }
        let picked: Vec<usize> = if idx.len() <= IDENT_POINTS {
            idx.clone()
        } else {
            let (f0, f1) = (frf.freqs[idx[0]], frf.freqs[*idx.last()?]);
            let mut out: Vec<usize> = (0..IDENT_POINTS)
                .map(|k| {
                    let f = f0 * (f1 / f0).powf(k as f64 / (IDENT_POINTS - 1) as f64);
                    let p = idx.partition_point(|&i| frf.freqs[i] < f);
                    idx[p.min(idx.len() - 1)]
                })
                .collect();
            out.dedup();
            out
        };
        let peak = idx.iter().map(|&i| frf.h_mag[i]).fold(0.0f64, f64::max);
        Some(IdentFigures {
            freqs_hz: picked.iter().map(|&i| frf.freqs[i]).collect(),
            gain: picked.iter().map(|&i| frf.h_mag[i]).collect(),
            coherence: picked.iter().map(|&i| frf.coherence[i]).collect(),
            trusted: picked
                .iter()
                .map(|&i| frf.trusted.get(i).copied().unwrap_or(false))
                .collect(),
            floor: self.plan.config.trust_h_floor_frac * peak,
            trust_coherence: self.plan.config.trust_coherence,
            unit: self.view.control_unit.clone(),
        })
    }

    fn ender(&self) -> String {
        self.shared
            .ended_by
            .lock()
            .ok()
            .and_then(|b| b.clone())
            .unwrap_or_else(|| "the operator".into())
    }

    fn stop_outcome(&self) -> &'static str {
        self.stop.as_ref().map_or("operator_stop", |(o, _)| *o)
    }

    fn fault(&mut self, words: &str) {
        self.shared.signal.store(ABORT, Ordering::SeqCst);
        if let Some(svc) = self.svc.as_mut() {
            let _ = svc.abort(words);
        }
        self.forward_log_now();
        self.end(
            "fault",
            "aborted",
            None,
            Some(format!("The run stopped on a fault: {words}")),
            None,
        );
    }

    // ---- review and engage --------------------------------------------------------

    fn proceed_cmd(&mut self) -> Result<(), MethodError> {
        if self.view.state != State::Review {
            return Err(wrong_state(&self.view, "Proceeding"));
        }
        self.shared.add_log(
            "note",
            "the operator reviewed the identification and proceeded",
        );
        self.proceed()
    }

    /// Build the law and engage at the start level.
    fn proceed(&mut self) -> Result<(), MethodError> {
        let svc = self
            .svc
            .as_mut()
            .ok_or_else(|| wrong_state(&self.view, "Proceeding"))?;
        let r = svc.proceed_after_review();
        let state = svc.state();
        self.forward_log_now();
        match r {
            Ok(()) => {
                self.engaged();
                Ok(())
            }
            // The law could not be built: the run ends before the tone starts.
            Err(_) if matches!(state, RunState::Stopped | RunState::Aborted) => {
                self.finish_from_controller();
                Ok(())
            }
            Err(e) => Err(refuse("wrong_state", e.to_string())),
        }
    }

    fn engaged(&mut self) {
        if let Some(svc) = self.svc.as_mut() {
            if self.plan.config.capture.rule == "entire_test" {
                match svc.start_capture(&self.plan.config.capture.run_name) {
                    Ok(dir) => self
                        .shared
                        .add_log("note", format!("recording to {}", dir.display())),
                    Err(e) => self
                        .shared
                        .add_log("note", format!("the recording did not start: {e}")),
                }
            }
        }
        self.law_state_init();
        self.set(State::Running, "engaged");
        self.refresh(true);
        self.publish();
    }

    /// The law's state before its first call: identity, the live values the run
    /// starts with, and the labels of its declared values.
    fn law_state_init(&mut self) {
        if self.view.law.is_some() {
            return;
        }
        let f = &self.plan.law;
        let live = f
            .live
            .iter()
            .map(|k| (k.clone(), self.param_value(k).unwrap_or(0.0)))
            .collect();
        self.view.law = Some(super::view::law_state(
            super::config::LawRef::of(f),
            live,
            Vec::new(),
        ));
    }

    /// A number parameter's value for the run: given, or its declared default.
    fn param_value(&self, key: &str) -> Option<f64> {
        if let Some(v) = self.plan.config.law.params.get(key).and_then(Value::as_f64) {
            return Some(v);
        }
        self.plan
            .law
            .params
            .iter()
            .find(|p| p.key == key)
            .and_then(|p| match &p.kind {
                hertzforge_law_host::ParamKind::Number(s) => Some(s.default),
                _ => None,
            })
    }

    // ---- the loop -----------------------------------------------------------------

    fn pump(&mut self) {
        if self.shared.signal.load(Ordering::SeqCst) == ABORT {
            self.abort_now(&self.ender());
            return;
        }
        self.t_before = self.view.t_s;
        let Some(svc) = self.svc.as_mut() else {
            self.fault("the run lost its station");
            return;
        };
        if !matches!(svc.state(), RunState::Engaged | RunState::FullLevel) {
            self.forward_log_now();
            self.refresh(false);
            self.finish_from_controller();
            return;
        }
        // One chunk (a quarter second of station time): the controller drains its
        // status, services its capture and advances its own state (full level, the
        // schedule's end) between chunks only.
        let r = svc.run_chunk();
        let mut last_block: Option<(Vec<f32>, usize)> = None;
        svc.service_analysis_tapped(|blk, stride| last_block = Some((blk.to_vec(), stride)));
        if let Some((blk, stride)) = last_block {
            self.keep_samples(&blk, stride);
        }
        match r {
            Ok(true) => {
                self.forward_log_now();
                self.refresh(false);
                self.pace();
                let due = self
                    .last_publish
                    .is_none_or(|t| t.elapsed() >= PUBLISH_PERIOD);
                if due {
                    self.refresh_figures();
                    self.publish();
                } else {
                    // A change of state is published at once.
                    let shown = self.shared.view();
                    if super::view::state_key(Some(&shown))
                        != super::view::state_key(Some(&self.view))
                    {
                        self.publish();
                    }
                }
            }
            Ok(false) => {
                self.forward_log_now();
                self.refresh(false);
                self.refresh_figures();
                self.finish_from_controller();
            }
            Err(e) => {
                if self.shared.signal.load(Ordering::SeqCst) == ABORT {
                    self.abort_now(&self.ender());
                } else {
                    self.fault(&format!("the loop failed: {e}"));
                }
            }
        }
    }

    /// Keep the last batch's samples of each enabled input (the acquired frame's
    /// order: the table's enabled inputs in table order).
    fn keep_samples(&mut self, blk: &[f32], stride: usize) {
        let inputs: Vec<u32> = self
            .table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .map(|c| c.index)
            .collect();
        let stride = stride.max(1);
        self.time = inputs
            .iter()
            .enumerate()
            .filter(|(k, _)| *k < stride)
            .map(|(k, &index)| {
                (
                    index,
                    blk.iter()
                        .skip(k)
                        .step_by(stride)
                        .copied()
                        .collect::<Vec<f32>>(),
                )
            })
            .collect();
    }

    /// Sleep while the station's time runs ahead of the wall clock on the simulator.
    fn pace(&mut self) {
        if self.plan.sim_speed <= 0.0 || self.view.station.kind != "sim" {
            return;
        }
        let Some(t) = self.view.t_s else { return };
        let (w0, t0) = *self.pace_origin.get_or_insert((Instant::now(), t));
        let ahead = (t - t0) / self.plan.sim_speed - w0.elapsed().as_secs_f64();
        if ahead > 0.002 {
            std::thread::sleep(Duration::from_secs_f64(ahead.min(0.25)));
        }
    }

    /// Read the controller's live status into the view.
    fn refresh(&mut self, entering: bool) {
        let Some(svc) = self.svc.as_ref() else { return };
        let st = svc.live_status();
        self.apply_status(&st, entering);
    }

    fn apply_status(&mut self, st: &SineStatus, entering: bool) {
        let cfg = self.plan.config.clone();
        if entering || st.rung_index != self.rung_base.0 {
            self.rung_base = (st.rung_index, st.sweeps_done);
        }
        let driving = matches!(st.state, RunState::Engaged | RunState::FullLevel);
        if self.view.state == State::Running && driving {
            let phase = if st.at_last_level {
                "full_level"
            } else {
                "engaged"
            };
            self.set(State::Running, phase);
        }
        self.view.t_s = Some(st.t_s);
        self.view.elapsed_s = Some(st.elapsed_test_s);
        self.view.at_level_s = Some(st.at_level_s);
        self.view.remaining_s = if self.view.mode == RunMode::Auto {
            st.remaining_test_s
        } else {
            None
        };
        self.view.frequency_hz = Some(st.freq_hz);
        self.view.direction = st.direction;
        self.view.held = st.held;
        self.view.level_db = Some(st.level_db);
        self.view.rung = Some(st.rung_index);
        self.view.achieved_db = st.achieved_level_db;
        self.view.target = (st.target_g > 0.0).then_some(st.target_g);
        self.view.measured = st.measured_g;
        self.view.drive_vpk = Some(st.drive_vpk);
        self.view.cola_vpk = st.cola_vpk;
        self.view.notch = st.notch_binding;
        let linear = matches!(
            &cfg.test,
            WireTest::Sweep {
                rate: SweepRate::LinearHzPerSec(_),
                ..
            } | WireTest::Rstd {
                rate: SweepRate::LinearHzPerSec(_),
                ..
            }
        );
        self.view.rate = if linear {
            Rate::HzPerS(st.sweep_rate_hz_per_s)
        } else {
            Rate::OctPerMin(st.sweep_rate_oct_per_min.unwrap_or(0.0))
        };
        self.view.sweeps = cfg.test.is_sweep().then(|| {
            let done = st.sweeps_done.saturating_sub(self.rung_base.1);
            let rung = if st.at_last_level {
                cfg.full_level
            } else {
                cfg.per_level
            };
            let of = match (self.view.mode, rung) {
                (RunMode::Auto, hertzforge_sine::RungFor::Sweeps(n)) => Some(n),
                _ => None,
            };
            (done, of)
        });
        for c in self.view.channels.iter_mut() {
            if let Some(i) = st.inputs.iter().find(|i| i.channel_index == c.index) {
                c.amplitude = i.tracked.is_finite().then_some(i.tracked.max(0.0));
                c.phase_deg = i.phase_deg.is_finite().then_some(i.phase_deg);
                c.rms = i.rms.is_finite().then_some(i.rms.max(0.0));
                c.overload = i.overload;
            }
        }
        let mut alarms = Vec::new();
        if st.alarm_active {
            if let (Some(m), t) = (st.measured_g, st.target_g) {
                if m > 0.0 && t > 0.0 {
                    let e = 20.0 * (m / t).log10();
                    alarms.push(Alarm::Band {
                        side: if e >= 0.0 { "high" } else { "low" },
                        error_db: e,
                    });
                }
            }
        }
        for c in &self.view.channels {
            if c.overload {
                alarms.push(Alarm::Overload { channel: c.index });
            }
        }
        if st.alarm_active && !self.alarm_was {
            if let Some(Alarm::Band { side, error_db }) = alarms.first() {
                self.shared.add_log(
                    "alarm",
                    format!(
                        "the control point is outside its alarm band ({side}, {}) at {} Hz",
                        db(*error_db),
                        num(super::config::round(st.freq_hz, 2))
                    ),
                );
            }
        }
        self.alarm_was = st.alarm_active;
        self.view.alarm = !alarms.is_empty();
        self.view.alarms = alarms;
        if let Some(l) = &st.law {
            let state = self.view.law.get_or_insert_with(|| {
                super::view::law_state(
                    super::config::LawRef::of(&self.plan.law),
                    Vec::new(),
                    Vec::new(),
                )
            });
            state.built = true;
            state.misses = l.misses;
            state.capped_ticks = l.capped_ticks;
            state.values = l.values.clone();
            if !l.live.is_empty() {
                state.live = l.live.clone();
            }
        }
        if self.view.identification.is_none() {
            self.view.identification = st.identification.as_ref().map(|i| IdentFacts {
                level_db: i.level_db,
                band_coherence: i.band_coherence,
                gate_coherence: i.gate_coherence,
                lag_frames: i.lag_frames,
                trusted_frac: i.trusted_fraction,
                passed: i.passed,
            });
        }
    }

    /// The figures the views are drawn from (it allocates: between batches, at the
    /// display's rate only).
    fn refresh_figures(&mut self) {
        let Some(svc) = self.svc.as_ref() else { return };
        let d = svc.display();
        let against_time = matches!(
            self.plan.config.test,
            WireTest::Dwell { .. } | WireTest::Rstd { .. }
        );
        self.view.figures = Some(Arc::new(self.figures_of(&d, against_time)));
    }

    fn figures_of(&self, d: &SineDisplay, against_time: bool) -> Figures {
        let v = |x: &[f32]| x.iter().map(|y| f64::from(*y)).collect::<Vec<f64>>();
        let limit_of = |index: u32, x: &[f64]| -> Option<Vec<f64>> {
            let l = self
                .plan
                .config
                .limit_channels
                .iter()
                .find(|l| l.channel == index)?;
            Some(
                x.iter()
                    .map(|f| {
                        l.profile
                            .abort_bounds(*f)
                            .map(|(_, hi)| hi)
                            .unwrap_or(f64::NAN)
                    })
                    .collect(),
            )
        };
        let mut f = if against_time {
            let c = &d.history;
            let x = d.history_t_s.clone();
            Figures {
                against_time: true,
                visited: vec![true; x.len()],
                reference: v(&c.target),
                alarm_lo: v(&c.alarm_lo),
                alarm_hi: v(&c.alarm_hi),
                abort_lo: v(&c.abort_lo),
                abort_hi: v(&c.abort_hi),
                measured: v(&c.measured),
                drive: v(&c.drive_vpk),
                cola: c.cola_vpk.as_deref().map(v),
                thd: v(&c.thd),
                inputs: c
                    .inputs
                    .iter()
                    .map(|i| InputCurves {
                        index: i.channel_index,
                        tracked: v(&i.tracked),
                        phase_deg: v(&i.phase_deg),
                        limit: i.limit.as_deref().map(v),
                    })
                    .collect(),
                x,
                ..Figures::default()
            }
        } else {
            let c = &d.sweep;
            let x = d.grid_hz.clone();
            Figures {
                against_time: false,
                visited: d.visited.clone(),
                reference: v(&d.reference),
                alarm_lo: v(&d.alarm_lo),
                alarm_hi: v(&d.alarm_hi),
                abort_lo: v(&d.abort_lo),
                abort_hi: v(&d.abort_hi),
                measured: v(&c.measured),
                drive: v(&c.drive_vpk),
                cola: c.cola_vpk.as_deref().map(v),
                thd: v(&c.thd),
                inputs: c
                    .inputs
                    .iter()
                    .map(|i| InputCurves {
                        index: i.channel_index,
                        tracked: v(&i.tracked),
                        phase_deg: v(&i.phase_deg),
                        limit: limit_of(i.channel_index, &x),
                    })
                    .collect(),
                x,
                ..Figures::default()
            }
        };
        f.time = self.time.clone();
        f.time_fs_hz = self.time_fs_hz;
        f.identification = self.identification.clone();
        f
    }

    // ---- the operator's verbs -------------------------------------------------------

    fn driving(&self) -> bool {
        self.view.state == State::Running && matches!(self.view.phase, "engaged" | "full_level")
    }

    /// Pump one batch so a command the controller takes on its next tick has
    /// taken effect, and read the status back.
    fn settle_command(&mut self) {
        if let Some(svc) = self.svc.as_mut() {
            if !matches!(svc.state(), RunState::Engaged | RunState::FullLevel) {
                return;
            }
            let alive = svc.run_chunk();
            svc.service_analysis_tapped(|_, _| {});
            self.forward_log_now();
            match alive {
                Ok(true) => self.refresh(false),
                Ok(false) => {
                    self.refresh(false);
                    self.finish_from_controller();
                }
                Err(e) => self.fault(&format!("the loop failed: {e}")),
            }
        }
    }

    fn level_cmd(&mut self, target: LevelTarget) -> Result<f64, MethodError> {
        if self.view.mode != RunMode::Manual {
            return Err(refuse(
                "manual_mode_required",
                "Level changes need the manual ladder; switch the ladder to manual first.",
            )
            .with_detail(json!({"mode": "auto"})));
        }
        if !self.driving() {
            return Err(wrong_state(&self.view, "A level change"));
        }
        let levels = self.view.levels_db.clone();
        let svc = self
            .svc
            .as_ref()
            .ok_or_else(|| wrong_state(&self.view, "A level change"))?;
        match target {
            LevelTarget::Delta(d) => svc.step_level(d),
            LevelTarget::To(to) => {
                match levels.iter().position(|l| (l - to).abs() < 1e-9) {
                    Some(i) => svc.jump_to_level(i),
                    None => return Err(MethodError::named(
                        ErrorCode::BadRequest,
                        "not_a_rung",
                        format!(
                            "{} dB is not a level of the schedule ({} dB).",
                            num(to),
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
                }
            }
        }
        .map_err(|e| refuse("wrong_state", e.to_string()))?;
        self.settle_command();
        let level = self.view.level_db.unwrap_or(0.0);
        self.shared
            .add_log("level", format!("level {} (operator)", db(level)));
        Ok(level)
    }

    fn mode_cmd(&mut self, mode: RunMode) -> Result<(), MethodError> {
        match self.view.state {
            State::Finished => Err(wrong_state(&self.view, "Changing the ladder mode")),
            State::Configured | State::Blocked => {
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
                self.shared.add_log(
                    "note",
                    format!("ladder mode {}", super::config::mode_name(mode)),
                );
                Ok(())
            }
        }
    }

    fn hold_cmd(&mut self, on: bool) -> Result<(), MethodError> {
        if !self.driving() {
            return Err(wrong_state(
                &self.view,
                if on { "Holding" } else { "Resuming" },
            ));
        }
        let svc = self
            .svc
            .as_ref()
            .ok_or_else(|| wrong_state(&self.view, "Holding"))?;
        if on { svc.hold() } else { svc.resume() }
            .map_err(|e| refuse("wrong_state", e.to_string()))?;
        self.shared.add_log(
            "note",
            if on {
                "held: the test's progress stops, the tone stays under control"
            } else {
                "resumed"
            },
        );
        self.settle_command();
        Ok(())
    }

    fn sweep_cmd(
        &mut self,
        verb: &str,
        act: impl FnOnce(&SineService) -> Result<f64, ServiceError>,
    ) -> Result<f64, MethodError> {
        if !self.plan.config.test.is_sweep() {
            return Err(not_a_sweep(self.plan.config.test.kind(), verb));
        }
        if !self.driving() {
            return Err(wrong_state(&self.view, verb));
        }
        let svc = self
            .svc
            .as_ref()
            .ok_or_else(|| wrong_state(&self.view, verb))?;
        let applied = act(svc).map_err(|e| refuse("wrong_state", e.to_string()))?;
        self.shared.add_log(
            "note",
            if applied > 0.0 {
                format!("{verb}: to {} Hz", num(super::config::round(applied, 2)))
            } else {
                verb.to_owned()
            },
        );
        self.settle_command();
        Ok(applied)
    }

    fn goto_cmd(&mut self, hz: f64) -> Result<f64, MethodError> {
        let band = self.plan.config.test.band();
        if self.plan.config.test.is_sweep() && !(hz >= band.0 && hz <= band.1) {
            return Err(refuse(
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
        self.sweep_cmd("A go to", |s| s.go_to(hz))
    }

    fn adjust_cmd(&mut self, key: &str, value: f64) -> Result<f64, MethodError> {
        if !(self.driving() || self.view.state == State::Review) {
            return Err(wrong_state(&self.view, "Adjusting a law parameter"));
        }
        let facts = &self.plan.law;
        let Some(spec) = facts.params.iter().find(|p| p.key == key) else {
            return Err(adjust_refused(
                key,
                "unknown_key",
                format!("{} declares no parameter \"{key}\".", facts.name),
            ));
        };
        if !facts.live.iter().any(|k| k == key) {
            return Err(adjust_refused(
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
            return Err(adjust_refused(
                key,
                "out_of_range",
                format!("{}'s {} {why}", facts.name, spec.label),
            ));
        }
        let view = &self.view;
        let svc = self
            .svc
            .as_mut()
            .ok_or_else(|| wrong_state(view, "Adjusting a law parameter"))?;
        let value = match svc.adjust(key, value) {
            Ok(v) => v,
            // The law is built at proceed: before it there is nothing to change.
            Err(AdjustRefusal::NotEngaged) => {
                return Err(wrong_state(view, "Adjusting a law parameter"))
            }
            Err(r) => {
                return Err(adjust_refused(
                    key,
                    r.why(),
                    format!("{}'s {}: {r}.", facts.name, spec.label),
                ))
            }
        };
        if let Some(l) = self.view.law.as_mut() {
            match l.live.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = value,
                None => l.live.push((key.to_owned(), value)),
            }
        }
        self.shared.add_log(
            "law",
            format!("{} set to {} (operator)", spec.label, num(value)),
        );
        if self.driving() {
            self.settle_command();
        }
        Ok(value)
    }

    // ---- stop and abort ---------------------------------------------------------------

    fn note_end_level(&mut self) {
        if self.end_level.is_none() {
            self.end_level = Some((self.view.level_db, self.view.frequency_hz));
        }
    }

    fn stop_cmd(&mut self, outcome: &'static str, by: String) {
        if !self.view.state.is_live() || self.view.state == State::Stopping {
            return;
        }
        self.stop = Some((outcome, by.clone()));
        self.note_end_level();
        self.shared.add_log(
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
        );
        match self.view.state {
            State::Armed | State::Review => self.end_stopped(outcome, None),
            State::Running if self.driving() => {
                let Some(svc) = self.svc.as_mut() else { return };
                let t = svc.live_status().t_s;
                let r = svc.request_stop();
                self.forward_log_now();
                if r.is_err() {
                    self.end_stopped(outcome, None);
                } else {
                    self.stop_at_t = Some(t);
                    self.set(State::Stopping, "ramping_down");
                }
            }
            // Identification and the gate end on the controller's cancel flag,
            // raised before this request was queued.
            _ => {}
        }
    }

    fn abort_now(&mut self, by: &str) {
        self.note_end_level();
        self.shared.signal.store(ABORT, Ordering::SeqCst);
        if let Some(svc) = self.svc.as_mut() {
            let _ = svc.abort(&format!("aborted by {by}"));
        }
        if let Ok(mut b) = self.shared.ended_by.lock() {
            if b.is_none() {
                *b = Some(by.to_owned());
            }
        }
        self.end("operator_abort", "aborted", None, None, None);
    }

    /// The run ended in a controlled stop: from `armed` or `review` at once, from
    /// identification on its cancel, or after a ramp.
    fn end_stopped(&mut self, outcome: &'static str, ramp_s: Option<f64>) {
        if let Some(svc) = self.svc.as_mut() {
            let _ = svc.stop();
        }
        self.end(outcome, "stopped", None, None, ramp_s);
    }

    /// The controller ended the run on this batch.
    fn finish_from_controller(&mut self) {
        let Some(svc) = self.svc.as_ref() else { return };
        let state = svc.state();
        let trip = svc.trip().cloned();
        let t_end = svc.live_status().t_s;
        match (state, trip) {
            // The law's own ends (a fault, or the change cap bound too long) take the
            // controlled stop: the ramp ran from the trip's tick to the end.
            (RunState::Stopped | RunState::Aborted, Some(t))
                if matches!(
                    t.kind,
                    TripKind::LawFault { .. } | TripKind::Divergence { .. }
                ) =>
            {
                let (json, message) = trip_facts(&t, &self.view.control_unit);
                self.shared.add_log("trip", message.clone());
                let t_trip = t.at_tick as f64 * self.view.grid.tick_s();
                let ramp = Some((t_end - t_trip).max(0.0));
                let outcome = if matches!(t.kind, TripKind::LawFault { .. }) {
                    "law_fault"
                } else {
                    "aborted"
                };
                self.end(outcome, "stopped", Some(json), Some(message), ramp);
            }
            (RunState::Aborted, Some(t)) => {
                let (json, message) = trip_facts(&t, &self.view.control_unit);
                self.shared.add_log("trip", message.clone());
                self.end("aborted", "aborted", Some(json), Some(message), None);
            }
            (RunState::Aborted, None) => {
                let words = svc
                    .end_reason()
                    .unwrap_or("the controller ended the run without a trip record")
                    .to_owned();
                self.end(
                    "fault",
                    "aborted",
                    None,
                    Some(format!("The run stopped on a fault: {words}")),
                    None,
                );
            }
            (RunState::Stopped, _) => {
                let ramp = self.stop_at_t.map(|s| (t_end - s).max(0.0));
                match self.stop.clone() {
                    Some((outcome, _)) => self.end(outcome, "stopped", None, None, ramp),
                    None => {
                        let words = match self.plan.config.end_s() {
                            Some(s) => format!(
                                "Test complete: the schedule ran its {} s of test time and the \
                                 drive ramped down with the controlled stop.",
                                num(super::config::round(s, 1))
                            ),
                            None => "Test complete: the schedule finished and the drive ramped \
                                     down with the controlled stop."
                                .to_owned(),
                        };
                        // The scheduler's end ramps the drive down inside the batch
                        // it finishes on: its length on the station's clock is that
                        // batch's, to within one batch.
                        let ramp = self.t_before.map(|s| (t_end - s).max(0.0));
                        self.end("completed", "stopped", None, Some(words), ramp)
                    }
                }
            }
            (RunState::CoherenceHold, _) => self.end(
                "held",
                "coherence_hold",
                None,
                Some("Coherence gate held; the run never drove at test level.".into()),
                None,
            ),
            (other, _) => self.fault(&format!("the loop ended in state {other:?}")),
        }
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
        let by = self
            .stop
            .as_ref()
            .map(|(_, b)| b.clone())
            .unwrap_or_else(|| self.ender());
        let was_driving = matches!(self.view.state, State::Running | State::Stopping)
            && matches!(self.view.phase, "engaged" | "full_level" | "ramping_down");
        let (level, at_hz) = match (&trip, self.end_level) {
            (Some(t), _) => (
                t.get("at_level_db").and_then(Value::as_f64),
                t.get("at_hz").and_then(Value::as_f64),
            ),
            (None, Some(l)) if was_driving => l,
            (None, _) if was_driving => (self.view.level_db, self.view.frequency_hz),
            _ => (None, None),
        };
        let at = match level {
            Some(l) => format!(
                "{} and {} Hz after {} s",
                db(l),
                num(super::config::round(at_hz.unwrap_or(0.0), 2)),
                num(super::config::round(self.view.t_s.unwrap_or(0.0), 2))
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
        self.forward_log_now();
        let unramped =
            ramp_s.is_none() && matches!(outcome, "aborted" | "operator_abort" | "fault");
        if unramped {
            self.shared.signal.store(ABORT, Ordering::SeqCst);
        }
        if let Some(svc) = self.svc.as_mut() {
            let _ = svc.stop_capture();
        }
        self.svc = None;
        self.lock = None;
        self.view.end = Some(End {
            outcome,
            message: message.clone(),
            at_s: self.view.t_s,
            level_db: level,
            at_hz,
            ramp_s: if unramped { None } else { ramp_s },
            trip,
        });
        self.set(State::Finished, phase);
        self.view.held = false;
        self.shared.add_log("note", message);
        self.publish();
    }

    // ---- the controller's log -------------------------------------------------------

    fn forward_log_now(&mut self) {
        if let Some(svc) = self.svc.take() {
            self.forward_log(&svc);
            self.svc = Some(svc);
        }
    }

    /// Copy the controller's new log events into the run log, every note (driver
    /// warnings included).
    fn forward_log(&mut self, svc: &SineService) {
        let events = svc.log().events();
        for e in events.iter().skip(self.forwarded) {
            let entry: Option<(&'static str, String)> = match &e.kind {
                EventKind::StateChange { .. } => None,
                EventKind::Feasibility { overall, .. } => {
                    Some(("feasibility", format!("feasibility: {overall}")))
                }
                EventKind::OverrideAck { .. } => None,
                EventKind::CoherenceGate {
                    band_coherence,
                    floor,
                    passed,
                } => Some((
                    "gate",
                    format!(
                        "coherence gate {}: band coherence {band_coherence:.3}, gate {floor:.2}",
                        if *passed { "passed" } else { "held" }
                    ),
                )),
                EventKind::LevelChange {
                    from_db,
                    to_db,
                    auto,
                } => (from_db != to_db).then(|| {
                    (
                        "level",
                        format!(
                            "level {} ({})",
                            db(*to_db),
                            if *auto { "schedule" } else { "operator" }
                        ),
                    )
                }),
                EventKind::ScalarAlarm { .. } => None,
                EventKind::RstdSearchDone { f_peak, q } => Some((
                    "note",
                    format!(
                        "resonance search found {} Hz, Q {}",
                        num(super::config::round(*f_peak, 2)),
                        num(super::config::round(*q, 1))
                    ),
                )),
                EventKind::RstdSetpointCaptured { phase_rad } => Some((
                    "note",
                    format!(
                        "phase set point captured at {} degrees",
                        num(super::config::round(phase_rad.to_degrees(), 1))
                    ),
                )),
                EventKind::RstdBandExit => Some(("note", "the resonance left the band".to_owned())),
                EventKind::Trip { .. } => None,
                // The view carries the law that was built; an operator's change is
                // logged where the verb is taken.
                EventKind::LawBuilt { .. } | EventKind::LawAdjust { .. } => None,
                EventKind::LawMisses { reason, count, .. } => Some((
                    "law",
                    format!("the law gave {count} unusable calls, the last for {reason}"),
                )),
                EventKind::CaptureStart { run_dir, .. } => {
                    Some(("note", format!("recording to {run_dir}")))
                }
                EventKind::CaptureStop { .. } | EventKind::Snapshot { .. } => None,
                EventKind::RateCoerced {
                    requested_hz,
                    actual_hz,
                } => Some((
                    "station",
                    format!(
                        "the station programmed {} Hz for the {} Hz asked",
                        num(super::config::round(*actual_hz, 3)),
                        num(super::config::round(*requested_hz, 3))
                    ),
                )),
                EventKind::BandClamped { from_hz, to_hz, .. } => Some((
                    "station",
                    format!(
                        "the band's top edge narrowed from {} to {} Hz, under 0.45 times the \
                         adopted rate",
                        num(super::config::round(*from_hz, 2)),
                        num(super::config::round(*to_hz, 2))
                    ),
                )),
                EventKind::Note { text } => Some(("note", text.clone())),
            };
            if let Some((kind, text)) = entry {
                self.shared.add_log(kind, text);
            }
        }
        self.forwarded = events.len();
    }
}

/// A live parameter's change at the controller.
/// The control rows' unit, for the views and messages.
fn control_unit(table: &ChannelTable) -> String {
    table
        .channels
        .iter()
        .find(|c| c.enabled && c.role == ChannelRole::Control)
        .map_or_else(|| "g".to_owned(), |c| unit_name(&c.units).to_string())
}

/// Every enabled input in table order, before the loop has measured anything.
pub fn live_channels(table: &ChannelTable) -> Vec<LiveChannel> {
    table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role != ChannelRole::Drive)
        .map(|c| LiveChannel {
            index: c.index,
            description: c.description.clone(),
            role: role_name(c.role),
            amplitude: None,
            phase_deg: None,
            rms: None,
            unit: unit_name(&c.units).to_string(),
            overload: false,
        })
        .collect()
}

/// `x` to three significant digits, trailing zeros kept.
fn sig3(x: f64) -> String {
    let r = super::profile::sig(x, 3);
    if r == 0.0 || !r.is_finite() {
        return format!("{:.2}", if r.is_finite() { 0.0 } else { r });
    }
    let e = r.abs().log10().floor() as i32;
    let d = (2 - e).max(0) as usize;
    format!("{r:.d$}")
}

/// A trip's facts on the wire, and its words.
pub fn trip_facts(t: &TripRecord, unit: &str) -> (Value, String) {
    let mut m = Map::new();
    let at = format!(
        "at {} Hz, {}",
        num(super::config::round(t.at_freq_hz, 2)),
        db(t.at_level_db)
    );
    let (kind, words) = match &t.kind {
        TripKind::ScalarAbort {
            side,
            measured_g,
            limit_g,
        } => {
            m.insert("side".into(), Value::from(side.as_str()));
            m.insert(
                "measured_g".into(),
                num(super::profile::sig(*measured_g, 4)),
            );
            m.insert("limit_g".into(), num(super::profile::sig(*limit_g, 4)));
            (
                "abort_band",
                format!(
                    "Aborted: the control point read {} {unit} {at}, outside its abort band \
                     ({side} side, {} {unit}).",
                    sig3(*measured_g),
                    sig3(*limit_g)
                ),
            )
        }
        TripKind::Kinematic { axis, value, limit } => {
            m.insert("axis".into(), Value::from(axis.as_str()));
            m.insert("value".into(), num(super::profile::sig(*value, 4)));
            m.insert("limit".into(), num(super::profile::sig(*limit, 4)));
            let u = if axis == "velocity" {
                "in/s"
            } else {
                "in pk-pk"
            };
            (
                "kinematic",
                format!(
                    "Aborted: the response's {axis} reached {} {u} {at}, past the shaker's {} \
                     {u}.",
                    sig3(*value),
                    sig3(*limit)
                ),
            )
        }
        TripKind::LimitChannel {
            channel_index,
            measured,
            limit,
        } => {
            m.insert("channel".into(), Value::from(*channel_index));
            m.insert("measured".into(), num(super::profile::sig(*measured, 4)));
            m.insert("limit".into(), num(super::profile::sig(*limit, 4)));
            (
                "limit_channel",
                format!(
                    "Aborted: limit channel {channel_index} read {} {at}, outside its abort \
                     band ({}).",
                    sig3(*measured),
                    sig3(*limit)
                ),
            )
        }
        TripKind::DriveVpk {
            realized_vpk,
            limit_vpk,
        } => {
            m.insert("vpk".into(), num(super::profile::sig(*realized_vpk, 4)));
            m.insert("abort_vpk".into(), num(super::profile::sig(*limit_vpk, 4)));
            (
                "drive_vpk",
                format!(
                    "Aborted: the drive reached {} V pk {at}, at or over its abort of {} V pk.",
                    sig3(*realized_vpk),
                    sig3(*limit_vpk)
                ),
            )
        }
        TripKind::ServoFault { consecutive_ticks } => {
            m.insert("consecutive_ticks".into(), Value::from(*consecutive_ticks));
            (
                "servo_fault",
                format!(
                    "Aborted: the control point read far under its target for \
                     {consecutive_ticks} ticks {at}."
                ),
            )
        }
        TripKind::WatchdogAccel {
            channel_index,
            axis,
            value,
            limit,
            measured,
            drive_scale,
            ..
        } => {
            m.insert("channel".into(), Value::from(*channel_index));
            m.insert("axis".into(), Value::from(axis.as_str()));
            m.insert("value".into(), num(super::profile::sig(*value, 4)));
            m.insert("limit".into(), num(super::profile::sig(*limit, 4)));
            m.insert("measured".into(), num(super::profile::sig(*measured, 4)));
            m.insert(
                "drive_vpk".into(),
                num(super::profile::sig(*drive_scale, 4)),
            );
            (
                "watchdog",
                format!(
                    "Aborted: channel {channel_index}'s raw samples reached {} {at}, past the \
                     watchdog's {}.",
                    sig3(*value),
                    sig3(*limit)
                ),
            )
        }
        TripKind::OscillatorRange {
            requested_hz,
            fs_hz,
        } => {
            m.insert("requested_hz".into(), num(*requested_hz));
            m.insert("fs_hz".into(), num(*fs_hz));
            (
                "oscillator_range",
                format!(
                    "Aborted: the tone was asked for {} Hz, outside what {} Hz can play.",
                    num(*requested_hz),
                    num(*fs_hz)
                ),
            )
        }
        TripKind::Divergence { capped_s } => {
            m.insert("capped_s".into(), num(super::config::round(*capped_s, 3)));
            (
                "divergence",
                format!(
                    "Aborted: the change cap held the law's drive back for {} s without a \
                     break {at}.",
                    num(super::config::round(*capped_s, 2))
                ),
            )
        }
        TripKind::LawFault { fault, consecutive } => {
            m.insert("reason".into(), Value::from(fault.reason()));
            m.insert("consecutive".into(), Value::from(*consecutive));
            (
                "law_fault",
                format!(
                    "The control law gave no usable drive {consecutive} times in a row \
                     ({fault}); the drive in force was held, then ramped down {at}."
                ),
            )
        }
    };
    let mut out = Map::new();
    out.insert("kind".into(), Value::from(kind));
    out.insert("at_hz".into(), num(super::config::round(t.at_freq_hz, 2)));
    out.insert(
        "at_level_db".into(),
        num(super::config::round(t.at_level_db, 2)),
    );
    out.insert("at_tick".into(), Value::from(t.at_tick));
    out.extend(m);
    (Value::Object(out), words)
}

#[cfg(test)]
mod tests {
    //! The run thread on the controller as it is before the law seam (its two
    //! configuration fields left out): the whole path from start to the end, the
    //! controlled stop, the abort, the review hold, the sweep verbs and the refusals.

    use super::*;
    use crate::station::DriverScanner;
    use serde_json::json;

    fn config(extra: impl FnOnce(&mut Value)) -> WireSine {
        let mut c = json!({
            "title": "run thread",
            "profile": {"rows": [{"freq_hz": 100, "quantity": "accel_g", "value": 0.2},
                                 {"freq_hz": 1000, "quantity": "accel_g", "value": 0.2}]},
            "test": {"kind": "sweep", "band_hz": [200, 800],
                     "rate": {"law": "log", "oct_per_min": 16}, "end": {"sweeps": 2}},
            "schedule": {"levels_db": [-6, 0], "per_level": {"sweeps": 1},
                         "full_level": {"sweeps": 1}},
            "identification": {"review": false},
            "limits": {"drive_vpk_ceiling": 5, "drive_vpk_abort": 8},
        });
        extra(&mut c);
        super::super::config::parse(&c).unwrap()
    }

    struct Rig {
        shared: Arc<RunShared>,
        tx: std::sync::mpsc::Sender<Cmd>,
        _thread: std::thread::JoinHandle<()>,
    }

    fn rig(w: WireSine) -> Rig {
        let facts = crate::laws::builtin_sine_facts();
        let table = crate::table::Table::new().table;
        let grid = super::super::config::Grid::of(w.fs_hz(), w.tick_ms, false);
        let initial = super::super::view::configured_view(
            "sine-0001",
            &w,
            grid,
            StationRef {
                kind: "sim",
                class: "sim",
                devices: Vec::new(),
            },
            None,
        );
        let (notify, _rx) = tokio::sync::mpsc::unbounded_channel();
        std::mem::forget(_rx);
        let shared = Arc::new(RunShared::new(initial.clone(), notify));
        let (tx, rx) = std::sync::mpsc::channel();
        let plan = RunPlan {
            run_id: "sine-0001".into(),
            config: Arc::new(w),
            law: facts,
            sim_speed: 0.0,
            capture_root: Some(std::env::temp_dir().join("sds-sine-run-tests")),
            timeout_s: 30.0,
        };
        let thread = spawn(plan, initial, shared.clone(), rx).unwrap();
        let _ = table;
        Rig {
            shared,
            tx,
            _thread: thread,
        }
    }

    fn ctx() -> ArmContext {
        ArmContext {
            table: crate::table::Table::new().table,
            station: StationPlan::Sim { fs_hz: 16384.0 },
            scanner: Arc::new(DriverScanner),
            data_root: std::env::temp_dir().join("sds-sine-run-tests"),
            bind: "ws://127.0.0.1:0/".into(),
            law: Ok(Arc::new(
                hertzforge_sine::law::servo_blend::ServoBlendFactory::new(
                    hertzforge_sine::law::servo_blend::ServoBlendParams::default(),
                )
                .unwrap(),
            )),
        }
    }

    fn ask<T>(r: &Rig, make: impl FnOnce(Reply<T>) -> Cmd) -> Result<T, MethodError> {
        let (tx, rx) = oneshot::channel();
        r.tx.send(make(tx)).unwrap();
        rx.blocking_recv().unwrap()
    }

    fn wait_until(r: &Rig, secs: u64, f: impl Fn(&SineView) -> bool) -> Arc<SineView> {
        let t0 = Instant::now();
        loop {
            let v = r.shared.view();
            if f(&v) {
                return v;
            }
            assert!(
                t0.elapsed() < Duration::from_secs(secs),
                "timed out in {} {} at t_s {:?}",
                v.state.name(),
                v.phase,
                v.t_s
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_run_goes_from_start_to_completed_with_the_ramp_on_the_stations_clock() {
        let r = rig(config(|_| {}));
        ask(&r, |reply| Cmd::Start { ctx: ctx(), reply }).unwrap();
        let v = wait_until(&r, 120, |v| v.state == State::Finished);
        let end = v.end.as_ref().unwrap();
        assert_eq!(end.outcome, "completed", "{}", end.message);
        let ramp = end.ramp_s.unwrap();
        assert!(ramp > 0.0, "the completion ramps: {ramp}");
        assert!(v.t_s.unwrap() > 15.0, "two sweeps of 7.5 s: {:?}", v.t_s);
        let (log, _) = r.shared.log_since(0);
        let text: Vec<&str> = log.iter().map(|e| e.text.as_str()).collect();
        assert!(
            text.iter().any(|t| t.contains("sysid to coherence gate")),
            "{text:?}"
        );
        assert!(text.iter().any(|t| t.contains("to full level")), "{text:?}");
    }

    #[test]
    fn a_stop_ramps_down_and_ends_operator_stop_at_the_level_it_was_asked() {
        // Manual, so the level cannot move on before the stop arrives.
        let r = rig(config(|c| {
            c["test"]["end"] = json!({"sweeps": 20});
            c["schedule"]["mode"] = json!("manual");
        }));
        ask(&r, |reply| Cmd::Start { ctx: ctx(), reply }).unwrap();
        wait_until(&r, 60, |v| {
            v.phase == "engaged" && v.t_s.is_some_and(|t| t > 2.0)
        });
        ask(&r, |reply| Cmd::Stop {
            outcome: "operator_stop",
            by: "the test".into(),
            reply: Some(reply),
        })
        .unwrap();
        let v = wait_until(&r, 60, |v| v.state == State::Finished);
        let end = v.end.as_ref().unwrap();
        assert_eq!(end.outcome, "operator_stop", "{}", end.message);
        assert_eq!(end.level_db, Some(-6.0));
        assert!(end.ramp_s.unwrap() > 0.0);
        assert!(end.message.contains("the test"), "{}", end.message);
    }

    #[test]
    fn an_abort_ends_without_the_ramp() {
        let r = rig(config(|c| c["test"]["end"] = json!({"sweeps": 20})));
        ask(&r, |reply| Cmd::Start { ctx: ctx(), reply }).unwrap();
        wait_until(&r, 60, |v| v.phase == "engaged");
        r.shared.signal_end(true, "an observer");
        ask(&r, |reply| Cmd::Abort {
            by: "an observer".into(),
            reply,
        })
        .unwrap();
        let v = wait_until(&r, 10, |v| v.state == State::Finished);
        let end = v.end.as_ref().unwrap();
        assert_eq!(end.outcome, "operator_abort");
        assert_eq!(end.ramp_s, None);
    }

    /// The review hold is where the operator reads the identification: its gain
    /// and coherence views are drawn from the run's figures as soon as it holds,
    /// before anything has driven at test level.
    #[test]
    fn the_identification_is_drawn_at_the_review_hold() {
        let r = rig(config(|c| {
            c["identification"]["review"] = json!(true);
        }));
        ask(&r, |reply| Cmd::Start { ctx: ctx(), reply }).unwrap();
        let v = wait_until(&r, 60, |v| v.state == State::Review);
        let figures = v.figures.as_deref();
        let mut arrays = crate::codec::Arrays::new();
        let views = super::super::view::identification_views(
            figures.and_then(|f| f.identification.as_ref()),
            &mut arrays,
        );
        let titles: Vec<&str> = views.iter().filter_map(|v| v["title"].as_str()).collect();
        assert_eq!(
            titles,
            ["Identification Gain", "Coherence"],
            "at the review hold"
        );
        ask(&r, |reply| Cmd::Abort {
            by: "test".into(),
            reply,
        })
        .unwrap();
    }

    #[test]
    fn the_review_hold_waits_for_proceed_and_the_sweep_verbs_act() {
        let r = rig(config(|c| {
            c["identification"]["review"] = json!(true);
            c["test"]["end"] = json!({"sweeps": 20});
            c["schedule"]["mode"] = json!("manual");
        }));
        ask(&r, |reply| Cmd::Start { ctx: ctx(), reply }).unwrap();
        let v = wait_until(&r, 60, |v| v.state == State::Review);
        assert_eq!(v.phase, "awaiting_review");
        assert!(v.identification.as_ref().is_some_and(|i| i.passed));
        // Nothing drives in review: a level change is refused.
        let e = ask(&r, |reply| Cmd::Level {
            target: LevelTarget::Delta(-1.0),
            reply,
        })
        .unwrap_err();
        assert_eq!(e.reason, Some("wrong_state"));
        ask(&r, |reply| Cmd::Proceed { reply }).unwrap();
        wait_until(&r, 30, |v| v.phase == "engaged");
        ask(&r, |reply| Cmd::Hold { on: true, reply }).unwrap();
        assert!(r.shared.view().held);
        let f = ask(&r, |reply| Cmd::GoTo { hz: 300.0, reply }).unwrap();
        assert!((f - 300.0).abs() < 1e-9);
        let at = r.shared.view().frequency_hz.unwrap();
        assert!(
            (at - 300.0).abs() < 0.5,
            "a go to lands within 0.5 Hz: {at}"
        );
        // Held, the frequency stays where it is over a chunk of station time.
        ask(&r, |reply| Cmd::Hold { on: true, reply }).unwrap();
        let after = r.shared.view().frequency_hz.unwrap();
        assert!((after - at).abs() < 0.01, "{at} then {after}");
        let e = ask(&r, |reply| Cmd::GoTo { hz: 900.0, reply }).unwrap_err();
        assert_eq!(e.reason, Some("out_of_band"));
        ask(&r, |reply| Cmd::Hold { on: false, reply }).unwrap();
        assert!(!r.shared.view().held);
        let level = ask(&r, |reply| Cmd::Level {
            target: LevelTarget::To(0.0),
            reply,
        })
        .unwrap();
        assert_eq!(level, 0.0);
        let e = ask(&r, |reply| Cmd::Level {
            target: LevelTarget::To(-3.0),
            reply,
        })
        .unwrap_err();
        assert_eq!(e.reason, Some("not_a_rung"));
        let e = ask(&r, |reply| Cmd::Adjust {
            key: "kp".into(),
            value: 0.4,
            reply,
        })
        .unwrap_err();
        assert_eq!(e.detail.as_ref().unwrap()["why"], "not_live");
        let e = ask(&r, |reply| Cmd::Adjust {
            key: "blend_w".into(),
            value: 2.0,
            reply,
        })
        .unwrap_err();
        assert_eq!(e.detail.as_ref().unwrap()["why"], "out_of_range");
        let e = ask(&r, |reply| Cmd::Adjust {
            key: "nope".into(),
            value: 1.0,
            reply,
        })
        .unwrap_err();
        assert_eq!(e.detail.as_ref().unwrap()["why"], "unknown_key");
        assert_eq!(
            ask(&r, |reply| Cmd::Adjust {
                key: "blend_w".into(),
                value: 0.2,
                reply,
            })
            .unwrap(),
            0.2
        );
        let live = r.shared.view().law.clone().unwrap().live;
        assert_eq!(live, vec![("blend_w".to_owned(), 0.2)]);
        ask(&r, |reply| Cmd::Stop {
            outcome: "operator_stop",
            by: "the test".into(),
            reply: Some(reply),
        })
        .unwrap();
        wait_until(&r, 30, |v| v.state == State::Finished);
    }

    #[test]
    fn the_capture_run_name_reaches_the_controllers_capture() {
        let r = rig(config(|c| {
            c["capture"] = json!({"rule": "entire_test", "run_name": "sine_name_reaches"});
        }));
        ask(&r, |reply| Cmd::Start { ctx: ctx(), reply }).unwrap();
        wait_until(&r, 120, |v| v.state == State::Finished);
        let (log, _) = r.shared.log_since(0);
        let rec = log
            .iter()
            .find(|e| e.text.starts_with("recording to"))
            .map(|e| e.text.clone())
            .unwrap_or_default();
        assert!(rec.contains("sine_name_reaches"), "{rec}");
        let dir = rec.trim_start_matches("recording to ").to_owned();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_dwell_refuses_the_sweep_verbs_by_name() {
        let r = rig(config(|c| {
            c["test"] = json!({"kind": "dwell", "freq_hz": 250, "end": {"seconds": 60}});
            c["schedule"]["per_level"] = json!({"seconds": 2});
            c["schedule"]["full_level"] = json!({"seconds": 2});
        }));
        ask(&r, |reply| Cmd::Start { ctx: ctx(), reply }).unwrap();
        wait_until(&r, 60, |v| v.phase == "engaged");
        for e in [
            ask(&r, |reply| Cmd::Reverse { reply }).unwrap_err(),
            ask(&r, |reply| Cmd::Jog {
                delta_hz: 5.0,
                reply,
            })
            .unwrap_err(),
            ask(&r, |reply| Cmd::GoTo { hz: 300.0, reply }).unwrap_err(),
        ] {
            assert_eq!(e.reason, Some("not_a_sweep"));
            assert_eq!(e.detail.as_ref().unwrap()["test"], "dwell");
        }
        let v = wait_until(&r, 10, |v| v.figures.is_some());
        assert!(v.figures.as_ref().is_some_and(|f| f.against_time));
        ask(&r, |reply| Cmd::Abort {
            by: "the test".into(),
            reply,
        })
        .unwrap();
    }
}
