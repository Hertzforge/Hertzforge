//! One configured SRS shock run, on its own thread, on the transient-run rails
//! ([`crate::pulse::rails`]).
//!
//! The thread owns the run's engine ([`super::engine`]) outright and drives it in
//! the order the protocol states: arm, identification, the review hold, the seed
//! and the notch, then pulses (in `auto` at the time between pulses, in `manual` on
//! `srs.fire`), and the end. Nothing here is control math.
//! Requests arrive as [`Cmd`]s the thread takes between pulses; while it waits for
//! the next pulse it keeps taking them. The thread publishes a view after every
//! step and puts each pulse's row and figures where the service reads them without
//! the thread.
//!
//! Stop and Abort reach a fire in flight through the run's cancel flag, raised by
//! the service before it queues the request. An absent controller raises the hold:
//! no pulse starts while it stands. Every engine call runs under `catch_unwind`; a
//! panic ends the run in the emergency stop. The station is held only while the
//! run is live.

use super::config::{Grid, WireSrs};
use super::engine::{
    ArmOutcome, ArmPlan, EngineContext, EngineEnd, EngineFactory, Fired, Identified, LevelMove,
    Seeded, SrsEngine,
};
use super::view::{End, SrsView, State, Trip};
use super::RunData;
use crate::protocol::{num, ErrorCode, MethodError};
use crate::pulse::rails::{self, guarded, Shared, Waited};
use crate::station::{NiScanner, StationPlan};
use hertzforge_channel_table::ChannelTable;
use hertzforge_shock_common::ShockRunMode;
use hertzforge_srs::SrsLawFactory;
use serde_json::json;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// Who stops a run when the service shuts down.
pub const SHUTDOWN_BY: &str = crate::random::run::SHUTDOWN_BY;

type Reply<T> = oneshot::Sender<Result<T, MethodError>>;

/// What a run arms on, taken from the service when the request arrives.
pub struct ArmContext {
    pub table: ChannelTable,
    pub station: StationPlan,
    pub scanner: Arc<dyn NiScanner>,
    pub data_root: PathBuf,
    pub capture_root: Option<PathBuf>,
    pub bind: String,
    pub law: Result<Arc<dyn SrsLawFactory>, MethodError>,
    /// The values the law reports, as its manifest declares them.
    pub law_values: Vec<hertzforge_law_host::DiagnosticSpec>,
}

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
    Fire {
        reply: Reply<()>,
    },
    Level {
        to: LevelMove,
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
    pub cfg: Arc<WireSrs>,
    pub engine: EngineFactory,
    pub sim_speed: f64,
    /// The orphan timeout, for the safe stop's message.
    pub timeout_s: f64,
}

fn refuse(reason: &'static str, message: impl Into<String>) -> MethodError {
    MethodError::named(ErrorCode::Refused, reason, message)
}

pub fn wrong_state(v: &SrsView, verb: &str) -> MethodError {
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

pub fn wrong_mode(verb: &str) -> MethodError {
    refuse(
        "wrong_mode",
        format!("{verb} needs the manual ladder; this run is automatic."),
    )
    .with_detail(json!({"mode": "auto"}))
}

/// The refusal of a pulse before the time between pulses has passed.
pub fn not_yet(seconds: f64) -> MethodError {
    refuse(
        "not_yet",
        format!(
            "The next pulse may fire in {} s, when the time between pulses has passed.",
            num(super::config::round(seconds.max(0.0), 2))
        ),
    )
    .with_detail(json!({"seconds": num(super::config::round(seconds.max(0.0), 3))}))
}

/// The run thread's state.
struct Actor {
    plan: RunPlan,
    shared: Arc<Shared<SrsView>>,
    data: Arc<RunData>,
    engine: Box<dyn SrsEngine>,
    view: SrsView,
    lock: Option<hertzforge_station::lock::LockGuard>,
    started: Option<Instant>,
    ended_at: Option<f64>,
    /// When the last pulse started, as the engine stamped it once the device's
    /// clock ran (this thread's own moment of asking when the engine keeps none):
    /// the next is due a spacing later.
    last_start: Option<Instant>,
    seeded: bool,
    stop_outcome: &'static str,
    stopped_by: String,
}

/// Start the run thread. Returns once the thread exists; the run is `configured`.
pub fn spawn(
    plan: RunPlan,
    initial: SrsView,
    shared: Arc<Shared<SrsView>>,
    data: Arc<RunData>,
    rx: Receiver<Cmd>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let name = format!("srs-{}", plan.run_id);
    std::thread::Builder::new().name(name).spawn(move || {
        let ctx = EngineContext {
            signals: shared.signals.clone(),
            progress: shared.progress.clone(),
            sim_speed: plan.sim_speed,
        };
        let engine = (plan.engine)(&ctx);
        let mut a = Actor {
            plan,
            shared,
            data,
            engine,
            view: initial,
            lock: None,
            started: None,
            ended_at: None,
            last_start: None,
            seeded: false,
            stop_outcome: "operator_stop",
            stopped_by: "the operator".into(),
        };
        a.run(rx);
    })
}

impl Actor {
    fn run(&mut self, rx: Receiver<Cmd>) {
        loop {
            match (self.view.state, self.view.phase) {
                (State::Running, "sysid") => self.identify(),
                (State::Running, _) => {
                    if !self.between_pulses(&rx) {
                        self.abandon();
                        return;
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
        if self.view.state.is_live() {
            self.shared.signals.raise(true);
            let _ = guarded(|| self.engine.abort("the service dropped the run"));
        }
        self.lock = None;
    }

    fn publish(&mut self) {
        if let Some(t0) = self.started {
            self.view.elapsed_s = Some(self.ended_at.unwrap_or_else(|| t0.elapsed().as_secs_f64()));
        }
        self.view.misses = self.engine.misses();
        // A finished run keeps the counts of its last pulse.
        if self.armed() || self.view.state == State::Finished {
            let p = self.engine.position();
            self.view.level_db = Some(p.level_db);
            self.view.rung = p.rung;
            self.view.pulses.at_level = p.at_level;
            self.view.pulses.of_level = p.of_level;
            self.view.pulses.passed = p.passed;
        }
        self.forward_log();
        self.shared.publish(&self.plan.run_id, self.view.clone());
    }

    fn armed(&self) -> bool {
        matches!(
            self.view.state,
            State::Armed | State::Running | State::Review | State::Stopping
        )
    }

    fn forward_log(&mut self) {
        for (kind, text) in self.engine.take_log() {
            self.shared.add_log(kind, text);
        }
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

    /// Run `f` on the engine; a panic ends the run in the emergency stop.
    fn call<T>(&mut self, what: &str, f: impl FnOnce(&mut dyn SrsEngine) -> T) -> Option<T> {
        let engine = &mut self.engine;
        match guarded(|| f(engine.as_mut())) {
            Ok(t) => Some(t),
            Err(words) => {
                self.shared.signals.raise(true);
                let engine = &mut self.engine;
                let _ = guarded(|| engine.abort("a panic in the run"));
                self.end(
                    "fault",
                    None,
                    Some(format!("The run stopped on a fault in {what}: {words}")),
                );
                None
            }
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
                let ok = self.view.state == State::Review;
                if !ok {
                    let _ = reply.send(Err(wrong_state(&self.view, "Proceeding")));
                    return;
                }
                self.shared
                    .add_log("note", "the operator reviewed identification and proceeded");
                let _ = reply.send(Ok(()));
                self.seed();
            }
            Cmd::Fire { reply } => match self.may_fire("A fire") {
                Err(e) => {
                    let _ = reply.send(Err(e));
                }
                Ok(()) => {
                    let _ = reply.send(Ok(()));
                    self.fire(|e| e.fire());
                }
            },
            Cmd::Level { to, reply } => {
                let r = self.level_cmd(to);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Stop { outcome, by, reply } => {
                if self.view.state.is_live() {
                    self.stop_outcome = outcome;
                    self.stopped_by = by;
                    self.end_on_stop();
                }
                if let Some(reply) = reply {
                    let _ = reply.send(Ok(()));
                }
            }
            Cmd::Abort { by, reply } => {
                if self.view.state.is_live() {
                    self.stopped_by = by;
                    self.end_on_abort();
                }
                let _ = reply.send(Ok(()));
            }
        }
    }

    // ---- arm and start ---------------------------------------------------------

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
        self.set(State::Running, "sysid");
        self.shared.add_log(
            "note",
            format!(
                "started; identification at {} dB, at most {} V pk",
                num(self.plan.cfg.sysid_level_db),
                num(super::config::round(
                    self.plan.cfg.identification_vpk_cap(),
                    4
                ))
            ),
        );
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

    fn arm(&mut self, ack: Option<String>, ctx: ArmContext) -> Result<(), MethodError> {
        let plan = ArmPlan {
            cfg: self.plan.cfg.clone(),
            table: ctx.table,
            station: ctx.station,
            scanner: ctx.scanner,
            data_root: ctx.data_root,
            capture_root: ctx.capture_root,
            bind: ctx.bind,
            law: ctx.law,
            law_values: ctx.law_values,
            ack: ack.clone(),
        };
        let Some(r) = self.call("arm", |e| e.arm(plan)) else {
            return Err(MethodError::internal("the run failed while arming"));
        };
        match r? {
            ArmOutcome::Blocked(feasibility) => {
                self.view.feasibility = Some(feasibility.clone());
                self.set(State::Blocked, "feasibility_blocked");
                Err(refuse(
                    "feasibility_exceeded",
                    "Feasibility exceeded: the pulse asks more of the shaker than its ratings. \
                     Review it, or arm with an acknowledgement.",
                )
                .with_detail(json!({
                    "feasibility": {"verdict": feasibility.0, "message": feasibility.1}
                })))
            }
            ArmOutcome::Armed(a) => {
                if let Some(ack) = &ack {
                    self.shared
                        .add_log("override", format!("operator acknowledgement: {ack}"));
                }
                self.view.feasibility = Some(a.feasibility.clone());
                self.view.grid = Grid {
                    fs_hz: a.fs_hz,
                    record_len: a.record_len,
                    actual: true,
                };
                self.view.clock = Some("shared");
                self.view.station = a.station;
                self.view.streaming = true;
                self.lock = a.lock;
                self.set(State::Armed, "armed");
                self.shared.add_log(
                    "station",
                    format!(
                        "armed on {} at {} Hz, records of {} samples",
                        self.view.station.class,
                        num(super::config::round(a.fs_hz, 3)),
                        a.record_len
                    ),
                );
                Ok(())
            }
        }
    }

    // ---- identification, review, seed --------------------------------------------

    fn identify(&mut self) {
        self.publish();
        let Some(r) = self.call("identification", |e| e.identify()) else {
            return;
        };
        if self.interrupted() {
            return;
        }
        match r {
            Err(e) => self.end("fault", None, Some(e.message)),
            Ok(Identified::Held { facts, message }) => {
                self.view.identification = facts;
                self.end("held", None, Some(message));
            }
            Ok(Identified::Done { facts, review }) => {
                self.view.identification = Some(facts);
                self.data.set_identification(self.engine.identification());
                if review {
                    self.set(State::Review, "awaiting_review");
                    self.publish();
                } else {
                    self.seed();
                }
            }
        }
    }

    fn seed(&mut self) {
        let Some(r) = self.call("the law's seed", |e| e.seed()) else {
            return;
        };
        if self.interrupted() {
            return;
        }
        match r {
            Err(e) => self.end("fault", None, Some(e.message)),
            Ok(Seeded::Ended(end)) => self.end_engine(end),
            Ok(Seeded::Ready(prediction)) => {
                self.view.prediction = Some(prediction);
                self.view.notch = self.engine.notch();
                self.seeded = true;
                self.data.set_law(self.engine.law_values());
                self.set(State::Running, "waiting");
                self.shared.add_log(
                    "note",
                    format!(
                        "seeded; the predicted full-level drive is {} V pk",
                        num(super::config::round(prediction.drive_vpk, 4))
                    ),
                );
                self.publish();
            }
        }
    }

    /// Ended by the operator's signal while a long call ran.
    fn interrupted(&mut self) -> bool {
        if self.shared.signals.aborting() {
            self.end_on_abort();
            true
        } else if self.shared.signals.cancelled() {
            self.end_on_stop();
            true
        } else {
            false
        }
    }

    // ---- between pulses and firing -----------------------------------------------

    fn spacing(&self) -> Duration {
        Duration::from_secs_f64(self.plan.cfg.spacing_s())
    }

    fn next_due(&self) -> Instant {
        self.last_start
            .map_or_else(Instant::now, |t| t + self.spacing())
    }

    /// Wait between pulses: take requests, and in `auto` fire when the next pulse is
    /// due. False when the service has dropped the run.
    fn between_pulses(&mut self, rx: &Receiver<Cmd>) -> bool {
        let auto = self.plan.cfg.mode == ShockRunMode::Auto;
        if !auto || !self.seeded {
            self.data.set_next_due(self.seeded.then(|| self.next_due()));
            match rx.recv_timeout(rails::POLL) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return false,
            }
            if self.view.state.is_live() && self.interrupted() {
                return true;
            }
            return true;
        }
        let due = self.next_due();
        self.data.set_next_due(Some(due));
        let signals = self.shared.signals.clone();
        let mut gone = false;
        let waited = rails::wait_until(due, &signals, || loop {
            match rx.try_recv() {
                Ok(cmd) => {
                    self.handle(cmd);
                    if self.view.state != State::Running {
                        return false;
                    }
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => {
                    gone = true;
                    return false;
                }
            }
        });
        if gone {
            return false;
        }
        match waited {
            Waited::Cancelled => {
                let _ = self.interrupted();
            }
            Waited::Interrupted => {}
            Waited::Due => self.fire(|e| e.fire()),
        }
        true
    }

    /// Whether a pulse may fire now on the operator's word.
    fn may_fire(&self, verb: &str) -> Result<(), MethodError> {
        if self.plan.cfg.mode == ShockRunMode::Auto {
            return Err(wrong_mode(verb));
        }
        if !(self.view.state == State::Running && self.view.phase == "waiting" && self.seeded) {
            return Err(wrong_state(&self.view, verb));
        }
        let left = self.next_due().saturating_duration_since(Instant::now());
        if !left.is_zero() {
            return Err(not_yet(left.as_secs_f64()));
        }
        Ok(())
    }

    fn fire(&mut self, f: impl FnOnce(&mut dyn SrsEngine) -> Result<Fired, MethodError>) {
        self.last_start = Some(Instant::now());
        self.data.set_next_due(None);
        self.set(State::Running, "firing");
        self.publish();
        let Some(r) = self.call("a pulse", f) else {
            return;
        };
        // The controller's own start time for the pulse: the next is due the time
        // between pulses after the device started, not after this thread asked.
        if let Some(t) = self.engine.last_pulse_start() {
            self.last_start = Some(t);
        }
        let fired = match r {
            Ok(f) => f,
            Err(e) => {
                self.end("fault", None, Some(e.message));
                return;
            }
        };
        self.view.pulses.fired += fired.rows.len() as u64;
        if let Some(last) = fired.rows.last() {
            self.view.alarm = last.alarm_bands.is_some_and(|n| n > 0);
            self.view.last_pulse = Some(last.clone());
        }
        self.data.add_pulses(fired.rows, fired.figures);
        self.data.set_law(self.engine.law_values());
        // The law moved the table: the prediction is the table in force's.
        if let Some(p) = self.engine.prediction() {
            self.view.prediction = Some(p);
        }
        if let Some(end) = fired.end {
            self.end_engine(end);
            return;
        }
        if self.interrupted() {
            return;
        }
        self.set(State::Running, "waiting");
        self.publish();
    }

    fn level_cmd(&mut self, to: LevelMove) -> Result<f64, MethodError> {
        if self.plan.cfg.mode == ShockRunMode::Auto {
            return Err(wrong_mode("A level change"));
        }
        let live = matches!(
            (self.view.state, self.view.phase),
            (State::Running, "waiting") | (State::Review, _)
        );
        if !live {
            return Err(wrong_state(&self.view, "A level change"));
        }
        let levels = &self.plan.cfg.levels_db;
        let (lo, hi) = (
            levels.first().copied().unwrap_or(0.0),
            levels.last().copied().unwrap_or(0.0),
        );
        let target = match to {
            LevelMove::Step(d) => self.engine.position().level_db + d,
            LevelMove::Rung(i) => *levels.get(i).ok_or_else(|| {
                MethodError::named(
                    ErrorCode::BadRequest,
                    "not_a_rung",
                    format!(
                        "The table has {} levels; rung {i} is not one.",
                        levels.len()
                    ),
                )
                .with_detail(
                    json!({"levels_db": levels.iter().map(|l| num(*l)).collect::<Vec<_>>()}),
                )
            })?,
        };
        if target < lo - 1e-9 || target > hi + 1e-9 {
            return Err(MethodError::named(
                ErrorCode::BadRequest,
                "not_a_rung",
                format!(
                    "{} dB is outside the table's levels ({} to {} dB).",
                    num(super::config::round(target, 2)),
                    num(lo),
                    num(hi)
                ),
            )
            .with_detail(
                json!({"levels_db": levels.iter().map(|l| num(*l)).collect::<Vec<_>>()}),
            ));
        }
        if let Some(vpk) = self.engine.predicted_vpk_at(target) {
            let abort = self.plan.cfg.drive_vpk_abort;
            if vpk > abort {
                return Err(refuse(
                    "drive_ceiling",
                    format!(
                        "The identified drive needs {} V pk at {} dB; the drive abort is {} V pk.",
                        num(super::config::round(vpk, 3)),
                        num(super::config::round(target, 2)),
                        num(abort)
                    ),
                )
                .with_detail(json!({
                    "vpk": num(super::config::round(vpk, 4)),
                    "abort_vpk": num(abort),
                    "level_db": num(super::config::round(target, 2)),
                })));
            }
        }
        let Some(r) = self.call("a level change", |e| e.set_level(to)) else {
            return Err(MethodError::internal("the run failed changing level"));
        };
        let level = r?;
        self.shared.add_log(
            "level",
            format!("level {} dB", num(super::config::round(level, 2))),
        );
        Ok(level)
    }

    // ---- the end --------------------------------------------------------------------

    fn end_on_stop(&mut self) {
        let by = self
            .shared
            .ended_by()
            .unwrap_or_else(|| self.stopped_by.clone());
        let outcome = self.shared.stop_outcome().unwrap_or(self.stop_outcome);
        let message = match outcome {
            "safe_stop" if by == SHUTDOWN_BY => {
                "The service was shut down; the run took its controlled stop.".to_owned()
            }
            "safe_stop" => format!(
                "Nobody held control for {} s; the run took its controlled stop.",
                num(self.plan.timeout_s.round())
            ),
            _ => format!("Stopped by {by}."),
        };
        let _ = guarded(|| self.engine.stop());
        self.end(outcome, None, Some(message));
    }

    fn end_on_abort(&mut self) {
        let by = self
            .shared
            .ended_by()
            .unwrap_or_else(|| self.stopped_by.clone());
        let engine = &mut self.engine;
        let _ = guarded(|| engine.abort("the operator aborted"));
        self.end("operator_abort", None, Some(format!("Aborted by {by}.")));
    }

    fn end_engine(&mut self, end: EngineEnd) {
        if end.outcome != "completed" {
            self.shared.add_log("trip", end.message.clone());
        }
        let _ = guarded(|| self.engine.stop());
        self.end(end.outcome, end.trip, Some(end.message));
    }

    fn end(&mut self, outcome: &'static str, trip: Option<Trip>, message: Option<String>) {
        if self.view.state == State::Finished {
            return;
        }
        let phase = match outcome {
            "aborted" | "operator_abort" | "fault" | "law_fault" => "aborted",
            "held" => "held",
            _ => "stopped",
        };
        self.ended_at = self.started.map(|t| t.elapsed().as_secs_f64());
        let message = message.unwrap_or_else(|| outcome.replace('_', " "));
        let pulse = (self.view.pulses.fired > 0).then(|| self.view.pulses.fired - 1);
        self.view.end = Some(End {
            outcome,
            message: message.clone(),
            at_s: self.ended_at,
            level_db: self.view.level_db,
            pulse,
            trip,
        });
        self.data.set_next_due(None);
        self.set(State::Finished, phase);
        self.shared
            .add_log("state", format!("ended ({outcome}): {message}"));
        self.publish();
        self.lock = None;
    }
}
