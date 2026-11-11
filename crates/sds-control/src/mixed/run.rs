//! The Mixed Mode run thread: it owns the controller's `MixedService`, takes the
//! operator's requests between the controller's chunks (about a quarter second of
//! station time each) and publishes the run's view. It never holds a lock shared
//! with the socket's threads across a chunk. Stop and abort are raised on the run's
//! signals first, so identification ends at its next block and an abort reaches
//! the station at once. When the run ends the service and the station are dropped
//! (the hardware freed) before the end is published.

use super::config::{LawFacts, WireMixed};
use super::view::{self, End, IdentFacts, LiveChannel, MixedView, State, StationRef};
use super::{refuse, wrong_state};
use crate::protocol::{num, MethodError};
use crate::random::backend::{Interruptible, ABORT, NONE};
use crate::random::run::{LevelTarget, LogEntry};
use crate::station::NiScanner;
use crate::station::{self, class_name, StationPlan};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_mixed::{
    LawRole, MixedAdjustRefusal, MixedConfig, MixedEventKind, MixedLawFactories, MixedOutcome,
    MixedService, MixedState, MixedStatus,
};
use hertzforge_random::RunMode;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// How often a running test publishes its view.
pub const PUBLISH_PERIOD: Duration = Duration::from_millis(200);
const SYSID_SEED: u64 = 0x5EED_0003;
pub const SHUTDOWN_BY: &str = crate::random::run::SHUTDOWN_BY;

/// A publish, as the service hears it: the run id and the view published.
pub type Notice = (String, Arc<MixedView>);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// What the run thread and the service share.
pub struct RunShared {
    view: Mutex<Arc<MixedView>>,
    log: Mutex<Vec<LogEntry>>,
    /// The station signal: [`ABORT`] takes the drive down with the station's own
    /// stop at the next engine block.
    pub signal: Arc<AtomicU8>,
    /// A stop asked from the socket's thread: identification ends at its next block.
    pub stop_asked: AtomicBool,
    /// The controller's cancel flag for identification, once the run has armed.
    pub sysid_cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// Who sent the stop or abort.
    pub ended_by: Mutex<Option<String>>,
    notify: tokio::sync::mpsc::UnboundedSender<Notice>,
    /// The `mixed.live` task has been started for this run.
    pub live_started: AtomicBool,
}

impl RunShared {
    pub fn new(
        initial: MixedView,
        notify: tokio::sync::mpsc::UnboundedSender<Notice>,
    ) -> RunShared {
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

    pub fn view(&self) -> Arc<MixedView> {
        lock(&self.view).clone()
    }

    pub fn log_since(&self, since: u64) -> (Vec<LogEntry>, u64) {
        let log = lock(&self.log);
        let next = log.len() as u64;
        let from = (since as usize).min(log.len());
        (log[from..].to_vec(), next)
    }

    pub fn add_log(&self, kind: &'static str, text: impl Into<String>) {
        let mut log = lock(&self.log);
        let seq = log.len() as u64;
        log.push(LogEntry {
            seq,
            at_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
            kind,
            text: text.into(),
        });
    }

    pub fn publish(&self, view: MixedView) {
        let id = view.run_id.clone();
        let view = Arc::new(view);
        *lock(&self.view) = view.clone();
        let _ = self.notify.send((id, view));
    }

    /// Ask the run to stop or abort from the socket's thread, before the request is
    /// queued.
    pub fn signal_end(&self, abort: bool, by: &str) {
        {
            let mut b = lock(&self.ended_by);
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
            if let Some(f) = lock(&self.sysid_cancel).clone() {
                f.store(true, Ordering::SeqCst);
            }
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
    /// The run's laws, resolved from the catalog; `Err` is why they could not be.
    pub laws: Result<MixedLawFactories, MethodError>,
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
    Part {
        id: String,
        on: bool,
        by: String,
        reply: Reply<bool>,
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
    pub wire: Arc<WireMixed>,
    pub core: Arc<MixedConfig>,
    pub law: LawFacts,
    /// Simulated seconds per wall second on the simulator; 0 runs unpaced (tests).
    pub sim_speed: f64,
    /// The capture root handed to the controller (`None`: its local default).
    pub capture_root: Option<PathBuf>,
    /// The orphan timeout, for the safe stop's message.
    pub timeout_s: f64,
}

struct Actor {
    plan: RunPlan,
    shared: Arc<RunShared>,
    svc: Option<MixedService>,
    lock: Option<hertzforge_station::lock::LockGuard>,
    view: MixedView,
    forwarded: usize,
    stop: Option<(&'static str, String)>,
    stop_at_t: Option<f64>,
    t_before: Option<f64>,
    last_publish: Option<Instant>,
    pace_origin: Option<(Instant, f64)>,
    /// Station time of the first engaged chunk; `elapsed_s` counts holds out.
    held_s: f64,
    last_t: Option<f64>,
    /// The level in force before any ramp down, for the end's `level_db`.
    driving_level: Option<f64>,
}

fn arm_refused(words: impl std::fmt::Display) -> MethodError {
    refuse("arm_refused", format!("The run cannot arm: {words}"))
}

/// Start the run thread. Returns once the thread exists; the run is `configured`.
pub fn spawn(
    plan: RunPlan,
    initial: MixedView,
    shared: Arc<RunShared>,
    rx: Receiver<Cmd>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let name = format!("mixed-{}", plan.run_id);
    std::thread::Builder::new().name(name).spawn(move || {
        let mut a = Actor {
            plan,
            shared,
            svc: None,
            lock: None,
            view: initial,
            forwarded: 0,
            stop: None,
            stop_at_t: None,
            t_before: None,
            last_publish: None,
            pace_origin: None,
            held_s: 0.0,
            last_t: None,
            driving_level: None,
        };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| a.run(rx)));
        if r.is_err() {
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

/// The enabled inputs of `table`, for the live frame.
pub fn live_channels(table: &ChannelTable) -> Vec<LiveChannel> {
    table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role != ChannelRole::Drive)
        .map(|c| LiveChannel {
            index: c.index,
            description: c.description.clone(),
            role: crate::table::role_name(c.role),
            rms: None,
            unit: crate::table::unit_name(&c.units),
            overload: false,
        })
        .collect()
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
            Cmd::Part { id, on, by, reply } => {
                let r = self.part_cmd(&id, on, &by);
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
    }

    fn arm_cmd(&mut self, ack: Option<String>, ctx: ArmContext) -> Result<(), MethodError> {
        match self.view.state {
            State::Configured | State::Blocked => self.arm(ack, ctx),
            State::Finished => Err(self.finished_refusal()),
            _ => Err(wrong_state(&self.view, "Arming")),
        }
    }

    fn start_cmd(&mut self, ctx: ArmContext) -> Result<(), MethodError> {
        match self.view.state {
            State::Configured => self.arm(None, ctx)?,
            State::Armed => {}
            State::Finished => return Err(self.finished_refusal()),
            _ => return Err(wrong_state(&self.view, "Starting")),
        }
        self.shared.stop_asked.store(false, Ordering::SeqCst);
        self.set(State::Running, "sysid");
        self.view.streaming = true;
        Ok(())
    }

    fn arm(&mut self, ack: Option<String>, ctx: ArmContext) -> Result<(), MethodError> {
        let mut laws = ctx.laws?;
        laws.floor_hinv.damping = self.plan.wire.random.control.damping;
        // The feasibility of the configuration as the core predicts it, against the
        // shaker's ratings: exceeded needs the operator's acknowledgement.
        let pred = hertzforge_mixed::predict(&self.plan.core).map_err(arm_refused)?;
        let shaker = self.plan.core.shaker.is_some();
        self.view.feasibility = Some(super::config::feasibility_of(&pred, shaker));
        if shaker && !pred.within_ratings() {
            match &ack {
                Some(a) => self
                    .shared
                    .add_log("override", format!("operator acknowledgement: {a}")),
                None => {
                    self.set(State::Blocked, "feasibility_blocked");
                    let f = self
                        .view
                        .feasibility
                        .as_ref()
                        .map(|(v, m)| json!({"verdict": v, "message": m}));
                    return Err(refuse(
                        "feasibility_exceeded",
                        "Feasibility exceeded: the test asks more of the shaker than its \
                         ratings. Review it, or arm with an acknowledgement.",
                    )
                    .with_detail(json!({ "feasibility": f })));
                }
            }
        }
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
        let svc = MixedService::new(
            (*self.plan.core).clone(),
            table.clone(),
            backend,
            class,
            SYSID_SEED,
            self.plan.capture_root.clone(),
        )
        .map_err(arm_refused)?;
        let svc = if hardware {
            svc
        } else {
            svc.with_block_frames(hertzforge_hal::default_block_frames(self.plan.wire.fs_hz()))
        };
        let mut svc = svc.with_laws(laws);
        svc.set_run_mode(self.view.mode).map_err(arm_refused)?;
        *lock_cancel(&self.shared) = Some(svc.sysid_cancel_flag());
        svc.arm().map_err(arm_refused)?;
        self.forward_log(&svc);
        let fs = svc.sample_rate_hz();
        let tick_frames = crate::sine::config::tick_frames(fs, self.plan.wire.tick_ms);
        self.view.grid.fs_hz = fs;
        self.view.grid.df_hz = fs / self.view.grid.n_pts.max(1) as f64;
        self.view.grid.tick_frames = tick_frames;
        self.view.grid.actual = true;
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
        self.svc = Some(svc);
        self.lock = lock;
        self.set(State::Armed, "armed");
        self.shared.add_log(
            "station",
            format!(
                "armed on {} at {} Hz, a {} frame tick, shared clock",
                class_name(class),
                num(super::config::round(fs, 3)),
                tick_frames
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
            if self.svc.as_ref().map(MixedService::state) == Some(MixedState::Aborted) {
                self.finish_from_controller();
            } else {
                self.fault(&format!("identification failed: {e}"));
            }
            return;
        }
        self.set(State::Running, "coherence_gate");
        self.refresh_display();
        self.publish();
        let Some(svc) = self.svc.as_mut() else { return };
        let gate = svc.coherence_gate();
        self.forward_log_now();
        self.ident_facts();
        match gate {
            Err(e) => self.fault(&format!("the coherence gate failed: {e}")),
            Ok(false) => {
                let c = self
                    .view
                    .identification
                    .as_ref()
                    .map_or(0.0, |i| i.band_coherence);
                let floor = self.plan.wire.random.control.coherence_floor;
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
            Ok(true) => {
                // The core always parks at review on a pass; without a review the run
                // proceeds at once.
                if self.plan.wire.random.control.review_after_sysid {
                    self.set(State::Review, "awaiting_review");
                    self.publish();
                } else if let Err(e) = self.proceed() {
                    self.fault(&e.message);
                }
            }
        }
    }

    fn ident_facts(&mut self) {
        let Some(svc) = self.svc.as_ref() else { return };
        let st = svc.live_status();
        let lag = svc.sysid_lag_frames();
        let c = &self.plan.wire.random.control;
        self.view.identification = Some(IdentFacts {
            level_db: c.pretest_level_db,
            band_coherence: st.band_coherence,
            gate_coherence: c.coherence_floor,
            lag_frames: lag,
            trusted_frac: st.trusted_frac,
            passed: st.band_coherence >= c.coherence_floor,
        });
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
            Err(_) if state.is_terminal() => {
                self.finish_from_controller();
                Ok(())
            }
            Err(e) => Err(refuse("wrong_state", e.to_string())),
        }
    }

    fn engaged(&mut self) {
        if let Some(svc) = self.svc.as_mut() {
            if self.plan.wire.capture.rule == "entire_test" {
                match svc.start_capture(&self.plan.wire.capture.run_name) {
                    Ok(dir) => self
                        .shared
                        .add_log("note", format!("recording to {}", dir.display())),
                    Err(e) => self
                        .shared
                        .add_log("note", format!("the recording did not start: {e}")),
                }
            }
        }
        if let Some(l) = self.view.law.as_mut() {
            l.built = true;
        }
        self.set(State::Running, "floor_only");
        self.refresh();
        self.publish();
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
        if svc.state().is_terminal() {
            self.forward_log_now();
            self.refresh();
            self.finish_from_controller();
            return;
        }
        let r = svc.run_chunk();
        // The time plots' tap is drained every chunk; the frames carry spectra only.
        svc.service_analysis_tapped(|_, _| {});
        match r {
            Ok(true) => {
                self.forward_log_now();
                self.refresh();
                self.pace();
                let due = self
                    .last_publish
                    .is_none_or(|t| t.elapsed() >= PUBLISH_PERIOD);
                let shown = self.shared.view();
                if due || view::state_key(Some(&shown)) != view::state_key(Some(&self.view)) {
                    self.refresh_display();
                    self.publish();
                }
            }
            Ok(false) => {
                self.forward_log_now();
                self.refresh();
                self.refresh_display();
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
    fn refresh(&mut self) {
        let Some(svc) = self.svc.as_ref() else { return };
        let st = svc.live_status();
        let stopping = self.view.state == State::Stopping;
        if st.state.is_terminal() {
            self.view.status = Some(st);
            return;
        }
        let was = self.view.phase;
        view::apply_status(&mut self.view, &st);
        if stopping {
            self.view.state = State::Stopping;
            self.view.phase = "ramping_down";
        }
        if self.view.phase != was {
            self.shared.add_log(
                "state",
                format!(
                    "{} to {}",
                    was.replace('_', " "),
                    self.view.phase.replace('_', " ")
                ),
            );
        }
        if !stopping && st.phase != hertzforge_mixed::MixedPhase::RampingDown {
            self.driving_level = Some(st.level_db);
        }
        if let (Some(last), true) = (self.last_t, st.held) {
            self.held_s += (st.t_s - last).max(0.0);
        }
        self.last_t = Some(st.t_s);
        self.view.elapsed_s = Some((st.t_s - self.held_s).max(0.0));
        self.view.tick_n = (st.t_s / self.view.grid.tick_s().max(1e-9)) as u64;
        self.view.loop_n = (st.t_s / self.view.grid.loop_s.max(1e-9)) as u64;
        if let Some(l) = self.view.law.as_mut() {
            let tones: Vec<&hertzforge_mixed::PartRow> = st
                .parts
                .iter()
                .filter(|r| r.kind == hertzforge_mixed::PartKind::Tone)
                .collect();
            l.tones.misses = tones.iter().map(|r| r.misses).max().unwrap_or(0);
            l.tones.capped = tones.iter().map(|r| r.capped).sum();
            if let Some(f) = st
                .parts
                .iter()
                .find(|r| r.kind == hertzforge_mixed::PartKind::Floor)
            {
                l.floor.misses = f.misses;
                l.floor.capped = f.capped;
            }
        }
        for c in &mut self.view.channels {
            c.overload = st.overloads.contains(&c.index);
            if let Some((_, rms)) = st.input_rms.iter().find(|(i, _)| *i == c.index) {
                c.rms = rms.is_finite().then_some(rms.max(0.0));
            }
        }
        self.view.status = Some(st);
    }

    fn refresh_display(&mut self) {
        if let Some(svc) = self.svc.as_ref() {
            self.view.display = Some(Arc::new(svc.display()));
        }
    }

    fn driving(&self) -> bool {
        self.view.state == State::Running
            && matches!(self.view.phase, "floor_only" | "engaged" | "full_level")
    }

    // ---- verbs --------------------------------------------------------------------

    fn level_cmd(&mut self, target: LevelTarget) -> Result<f64, MethodError> {
        if !self.driving() {
            return Err(wrong_state(&self.view, "A level change"));
        }
        let svc = self
            .svc
            .as_ref()
            .ok_or_else(|| wrong_state(&self.view, "A level change"))?;
        if svc.run_mode() != RunMode::Manual {
            return Err(refuse(
                "manual_mode_required",
                "The level moves by hand only in manual mode; switch with mixed.mode first.",
            )
            .with_detail(json!({"mode": "auto"})));
        }
        let now = svc.live_status().level_db;
        let levels = &self.view.levels_db;
        let r = match target {
            LevelTarget::Delta(d) => svc.step_level(d).map(|_| now + d),
            LevelTarget::To(t) => match levels.iter().position(|l| (l - t).abs() < 1e-9) {
                Some(i) => svc.jump_to_level(i).map(|_| t),
                None => {
                    return Err(refuse(
                        "not_a_rung",
                        format!("{} dB is not a level of the schedule.", num(t)),
                    )
                    .with_detail(json!({"levels_db": levels})))
                }
            },
        };
        let level = r.map_err(|e| refuse("wrong_state", e.to_string()))?;
        self.shared.add_log(
            "level",
            format!("level to {} dB", num(super::config::round(level, 2))),
        );
        Ok(level)
    }

    fn mode_cmd(&mut self, mode: RunMode) -> Result<(), MethodError> {
        if self.view.state == State::Finished {
            return Err(self.finished_refusal());
        }
        if let Some(svc) = self.svc.as_mut() {
            svc.set_run_mode(mode)
                .map_err(|e| refuse("wrong_state", e.to_string()))?;
        }
        self.view.mode = mode;
        Ok(())
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
        self.view.held = on;
        self.shared.add_log(
            "hold",
            if on {
                "progress held"
            } else {
                "progress resumed"
            },
        );
        Ok(())
    }

    fn part_refused(id: &str, why: &'static str, words: String) -> MethodError {
        refuse("part_refused", words).with_detail(json!({"id": id, "why": why}))
    }

    fn part_cmd(&mut self, id: &str, on: bool, by: &str) -> Result<bool, MethodError> {
        let Some(p) = self.plan.core.parts.iter().find(|p| p.id == id) else {
            return Err(Self::part_refused(
                id,
                "unknown_part",
                format!("The test has no part \"{id}\"."),
            ));
        };
        if p.kind() == "floor" && !on {
            return Err(Self::part_refused(
                id,
                "floor",
                "The floor plays for the whole run; it cannot be switched off.".into(),
            ));
        }
        if !matches!(self.view.phase, "engaged" | "full_level") || self.view.state != State::Running
        {
            return Err(wrong_state(&self.view, "Switching a part"));
        }
        let row = self
            .view
            .status
            .as_ref()
            .and_then(|s| s.parts.iter().find(|r| r.id == id).cloned());
        if let Some(r) = &row {
            if r.enabled == on {
                return Ok(on);
            }
            if on && r.done {
                let parts_s = self.view.parts_s.unwrap_or(0.0);
                let closed = p.window.off_s.is_some_and(|off| parts_s >= off);
                return Err(if closed {
                    Self::part_refused(
                        id,
                        "window_closed",
                        format!("Part {id}'s window has closed."),
                    )
                } else {
                    Self::part_refused(
                        id,
                        "schedule_ended",
                        format!("Part {id}'s schedule has ended."),
                    )
                });
            }
        }
        let svc = self
            .svc
            .as_ref()
            .ok_or_else(|| wrong_state(&self.view, "Switching a part"))?;
        svc.set_part_enabled(id, on)
            .map_err(|e| refuse("wrong_state", e.to_string()))?;
        self.shared.add_log(
            "part",
            format!(
                "part {id} switched {} by {by}",
                if on { "on" } else { "off" }
            ),
        );
        Ok(on)
    }

    fn adjust_cmd(&mut self, key: &str, value: f64) -> Result<f64, MethodError> {
        let Some(svc) = self.svc.as_mut() else {
            return Err(wrong_state(&self.view, "An adjustment"));
        };
        match svc.adjust(LawRole::Tones, key, value) {
            Ok(v) => {
                if let Some(l) = self.view.law.as_mut() {
                    match l.tones.live.iter_mut().find(|(k, _)| k == key) {
                        Some(slot) => slot.1 = v,
                        None => l.tones.live.push((key.to_owned(), v)),
                    }
                }
                self.shared
                    .add_log("law", format!("tone law {key} to {}", num(v)));
                Ok(v)
            }
            Err(MixedAdjustRefusal::NotEngaged) => Err(wrong_state(&self.view, "An adjustment")),
            Err(e) => Err(refuse("adjust_refused", e.to_string())
                .with_detail(json!({"key": key, "why": e.why()}))),
        }
    }

    // ---- the end ------------------------------------------------------------------

    fn stop_cmd(&mut self, outcome: &'static str, by: String) {
        if !self.view.state.is_live() || self.view.state == State::Stopping {
            return;
        }
        self.stop = Some((outcome, by.clone()));
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
            _ => {}
        }
    }

    fn abort_now(&mut self, by: &str) {
        self.shared.signal.store(ABORT, Ordering::SeqCst);
        if let Some(svc) = self.svc.as_mut() {
            let _ = svc.abort(&format!("aborted by {by}"));
        }
        {
            let mut b = lock(&self.shared.ended_by);
            if b.is_none() {
                *b = Some(by.to_owned());
            }
        }
        self.end("operator_abort", "aborted", None, None, None);
    }

    fn end_stopped(&mut self, outcome: &'static str, ramp_s: Option<f64>) {
        if let Some(svc) = self.svc.as_mut() {
            let _ = svc.stop();
        }
        self.end(outcome, "stopped", None, None, ramp_s);
    }

    /// The controller ended the run.
    fn finish_from_controller(&mut self) {
        let Some(svc) = self.svc.as_ref() else { return };
        let st: MixedStatus = svc.live_status();
        let t_end = st.t_s;
        let level = st.level_db;
        let trip = svc.trip().cloned();
        let names = super::config::part_names(&self.plan.wire.parts);
        let name_of = |id: &str| {
            self.plan
                .wire
                .parts
                .iter()
                .position(|p| p.id == id)
                .map(|i| names[i].clone())
                .unwrap_or_else(|| id.to_owned())
        };
        let trip_value = trip.as_ref().map(|t| {
            let at_hz = t
                .part
                .as_deref()
                .and_then(|id| st.parts.iter().find(|r| r.id == id).and_then(|r| r.freq_hz));
            let mut v = view::trip_json(t, level, at_hz);
            // The floor's own trips name the floor.
            if v["kind"] == "envelope" && v["part"].is_null() {
                if let Some(f) = self.plan.core.parts.iter().find(|p| p.kind() == "floor") {
                    v["part"] = Value::from(f.id.as_str());
                }
            }
            v
        });
        let trip_words = trip.as_ref().map(|t| {
            let w = t.words();
            match &t.part {
                Some(id) => w.replacen(&format!(" {id} "), &format!(" {} ", name_of(id)), 1),
                None => w,
            }
        });
        // An abort the operator (or the service) raised takes the station down with
        // its own stop; whatever the core records of that is the abort's, not a trip.
        if self.shared.signal.load(Ordering::SeqCst) == ABORT && self.stop.is_none() {
            self.end("operator_abort", "aborted", None, None, None);
            return;
        }
        match (st.outcome, st.state) {
            (Some(MixedOutcome::LawFault), _) => {
                let ramp = trip.as_ref().map(|t| (t_end - t.t_s).max(0.0));
                self.end("law_fault", "stopped", trip_value, trip_words, ramp)
            }
            (Some(MixedOutcome::Aborted), _) | (_, MixedState::Aborted) if trip.is_some() => {
                self.end("aborted", "aborted", trip_value, trip_words, None)
            }
            (_, MixedState::Aborted) => {
                if self.shared.signal.load(Ordering::SeqCst) == ABORT {
                    self.end("operator_abort", "aborted", None, None, None)
                } else {
                    let words = svc
                        .end_reason()
                        .unwrap_or_else(|| "the controller ended the run without a trip".into());
                    self.end(
                        "fault",
                        "aborted",
                        None,
                        Some(format!("The run stopped on a fault: {words}")),
                        None,
                    )
                }
            }
            (_, MixedState::CoherenceHold) => self.end(
                "held",
                "coherence_hold",
                None,
                Some("Coherence gate held; the run never drove at test level.".into()),
                None,
            ),
            (_, MixedState::Stopped) => {
                let ramp = self
                    .stop_at_t
                    .or(self.t_before)
                    .map(|s| (t_end - s).max(0.0));
                match self.stop.clone() {
                    Some((outcome, _)) => self.end(outcome, "stopped", None, None, ramp),
                    None => self.end(
                        "completed",
                        "stopped",
                        None,
                        Some(
                            "Test complete: the schedule finished and every part ramped down \
                             with the controlled stop."
                                .into(),
                        ),
                        ramp,
                    ),
                }
            }
            (_, other) => self.fault(&format!("the loop ended in state {}", other.name())),
        }
    }

    /// End the run, release the service and the station, then say how it ended.
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
        let level = self.driving_level.or(self.view.level_db);
        let at = match level {
            Some(l) => format!(
                "{} dB after {} s",
                num(super::config::round(l, 2)),
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
        // The release order: the service and the station go, then the end is said.
        self.svc = None;
        self.lock = None;
        self.view.end = Some(End {
            outcome,
            message: message.clone(),
            at_s: self.view.t_s,
            level_db: level,
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

    /// The controller's events since the last call, into the run log; a driver
    /// warning reaches it as the controller wrote it.
    fn forward_log(&mut self, svc: &MixedService) {
        let events = svc.log().events();
        for e in events.iter().skip(self.forwarded) {
            let (kind, text) = match &e.kind {
                MixedEventKind::State { .. } | MixedEventKind::Phase { .. } => continue,
                MixedEventKind::Level { level_db } => (
                    "level",
                    format!("level {} dB", num(super::config::round(*level_db, 2))),
                ),
                MixedEventKind::Part { part, on } => (
                    "part",
                    format!("part {part} {}", if *on { "on" } else { "off" }),
                ),
                MixedEventKind::Hold { held } => (
                    "hold",
                    if *held {
                        "held".into()
                    } else {
                        "resumed".into()
                    },
                ),
                MixedEventKind::Trip { trip } => ("trip", trip.words()),
                MixedEventKind::LawMiss { part, reason } => (
                    "law",
                    match part {
                        Some(p) => format!("tone {p}'s law call not used: {reason}"),
                        None => format!("the floor law's call not used: {reason}"),
                    },
                ),
                MixedEventKind::Note { text } => ("note", text.clone()),
            };
            self.shared.add_log(kind, text);
        }
        self.forwarded = events.len();
    }
}

fn lock_cancel(s: &RunShared) -> std::sync::MutexGuard<'_, Option<Arc<AtomicBool>>> {
    lock(&s.sysid_cancel)
}
