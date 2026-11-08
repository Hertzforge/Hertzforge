//! One configured TWR run, on its own thread, on the transient-run rails
//! ([`crate::pulse::rails`]).
//!
//! The thread owns the run's engine ([`super::engine`]) outright and drives it in
//! the order the protocol states: arm, identification, the review hold (or the
//! coherence hold), proceed, then plays (in `auto` one after another, in `manual`
//! on `twr.iterate`, and through the playback once `twr.play` has begun it), and
//! the end. Nothing here is control math. Requests arrive as [`Cmd`]s the thread
//! takes between plays; while it waits it keeps taking them. The thread publishes
//! a view after every step and puts each play's row and records where the service
//! reads them without the thread.
//!
//! Stop and Abort reach a play in flight through the run's cancel flag, raised by
//! the service before it queues the request. An absent controller raises the hold:
//! no play starts while it stands, and the play in flight finishes. Every engine
//! call runs under `catch_unwind`; a panic ends the run in the emergency stop. The
//! station is held only while the run is live.

use super::config::{Grid, WireTwr};
use super::engine::{
    level_words, ArmOutcome, ArmPlan, EngineContext, EngineEnd, EngineFactory, Identified,
    LevelMove, Proceeded, TwrEngine,
};
use super::reference::Stored;
use super::view::{End, State, Trip, TwrView};
use super::{RunData, Starting};
use crate::protocol::{num, ErrorCode, MethodError};
use crate::pulse::rails::{self, guarded, Shared, Waited};
use crate::shock::config::LawFacts;
use crate::station::{NiScanner, StationPlan};
use hertzforge_channel_table::ChannelTable;
use hertzforge_twr::schedule::TwrRunMode;
use hertzforge_twr::TwrLawFactory;
use serde_json::json;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::oneshot;

/// Who stops a run when the service shuts down.
pub const SHUTDOWN_BY: &str = crate::random::run::SHUTDOWN_BY;

/// A publish, as the service hears it.
pub type Notice = rails::Notice<TwrView>;

type Reply<T> = oneshot::Sender<Result<T, MethodError>>;

/// What a run arms on, taken from the service when the request arrives.
pub struct ArmContext {
    pub table: ChannelTable,
    pub station: StationPlan,
    pub scanner: Arc<dyn NiScanner>,
    pub data_root: PathBuf,
    pub capture_root: Option<PathBuf>,
    pub bind: String,
    pub stored: Arc<Stored>,
    pub facts: LawFacts,
    pub law: Result<Arc<dyn TwrLawFactory>, MethodError>,
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
    Iterate {
        reply: Reply<()>,
    },
    Play {
        reply: Reply<()>,
    },
    Level {
        to: LevelMove,
        reply: Reply<f64>,
    },
    Freeze {
        frozen: bool,
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
    pub cfg: Arc<WireTwr>,
    pub engine: EngineFactory,
    pub sim_speed: f64,
    /// The orphan timeout, for the safe stop's message.
    pub timeout_s: f64,
}

fn refuse(reason: &'static str, message: impl Into<String>) -> MethodError {
    MethodError::named(ErrorCode::Refused, reason, message)
}

pub fn wrong_state(v: &TwrView, verb: &str) -> MethodError {
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

/// The refusal at the coherence hold, in the service's own words.
pub fn coherence_hold(message: &str) -> MethodError {
    refuse(
        "coherence_hold",
        format!(
            "The run is held at the coherence hold: {} Only twr.reset releases it.",
            message
        ),
    )
}

/// The refusal of playback before the present level has gated.
pub fn not_converged(level_db: f64) -> MethodError {
    refuse(
        "not_converged",
        format!(
            "Playback begins once the present level has gated; {} has not.",
            level_words(level_db)
        ),
    )
    .with_detail(json!({"level_db": num(crate::shock::config::round(level_db, 2))}))
}

/// The run thread's state.
struct Actor {
    plan: RunPlan,
    shared: Arc<Shared<TwrView>>,
    data: Arc<RunData>,
    engine: Box<dyn TwrEngine>,
    view: TwrView,
    lock: Option<hertzforge_station::lock::LockGuard>,
    seeded: bool,
    hold_message: String,
    /// The play a stop or an abort cancelled, and the frames it had played.
    cancelled: Option<(u64, u64)>,
    stop_outcome: &'static str,
    stopped_by: String,
}

/// Start the run thread. Returns once the thread exists; the run is `configured`.
pub fn spawn(
    plan: RunPlan,
    initial: TwrView,
    shared: Arc<Shared<TwrView>>,
    data: Arc<RunData>,
    rx: Receiver<Cmd>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let name = format!("twr-{}", plan.run_id);
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
            seeded: false,
            hold_message: String::new(),
            cancelled: None,
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
                    if !self.between_plays(&rx) {
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

    fn armed(&self) -> bool {
        self.view.state.is_live() || self.view.state == State::Finished
    }

    fn publish(&mut self) {
        if self.armed() && self.view.grid.actual {
            let p = self.engine.position();
            self.view.level_db = Some(p.level_db);
            self.view.rung = p.rung;
            self.view.plays.at_level = p.at_level;
            self.view.plays.of_level = p.of_level;
            self.view.plays.playback = p.playback;
            self.view.plays.of_playback = p.of_playback;
            self.view.frozen = p.frozen;
            self.view.alarm = self.view.alarm || p.alarm;
            let fs = self.view.grid.fs_hz;
            if fs > 0.0 {
                self.view.t_s = Some(self.engine.frames_clocked() as f64 / fs);
            }
            if let Some(l) = self.view.law.as_mut() {
                l.misses = self.engine.misses();
            }
            if self.seeded && self.view.state.is_live() {
                if let Some(vpk) = self.engine.predicted_vpk_at(p.level_db) {
                    self.view.prediction = Some(super::view::Prediction {
                        drive_vpk: vpk,
                        drive_vpk_ceiling: self.plan.cfg.drive_vpk_ceiling,
                        level_db: p.level_db,
                    });
                }
            }
        }
        self.forward_log();
        self.shared.publish(&self.plan.run_id, self.view.clone());
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
    fn call<T>(&mut self, what: &str, f: impl FnOnce(&mut dyn TwrEngine) -> T) -> Option<T> {
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
            Cmd::Proceed { reply } => match self.view.state {
                State::Review => {
                    self.shared
                        .add_log("note", "the operator reviewed identification and proceeded");
                    let _ = reply.send(Ok(()));
                    self.proceed();
                }
                State::Held => {
                    let _ = reply.send(Err(coherence_hold(&self.hold_message)));
                }
                _ => {
                    let _ = reply.send(Err(wrong_state(&self.view, "Proceeding")));
                }
            },
            Cmd::Iterate { reply } => match self.may_iterate() {
                Err(e) => {
                    let _ = reply.send(Err(e));
                }
                Ok(()) => {
                    let _ = reply.send(Ok(()));
                    self.play_once();
                }
            },
            Cmd::Play { reply } => {
                let r = self.play_cmd();
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Level { to, reply } => {
                let r = self.level_cmd(to);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Freeze { frozen } => {
                if matches!(self.view.state, State::Running | State::Review) {
                    let _ = self.call("freezing", |e| e.freeze(frozen));
                    self.view.frozen = frozen;
                    self.shared.add_log(
                        "note",
                        if frozen {
                            "the law's corrections are frozen; the drive in force replays"
                        } else {
                            "the law's corrections resume"
                        },
                    );
                    self.publish();
                }
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
        self.set(State::Running, "sysid");
        self.shared.add_log(
            "note",
            format!(
                "started; identification at {} V rms for {} s",
                num(self.plan.cfg.sysid_drive_vrms),
                num(self.plan.cfg.sysid_duration_s)
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
            stored: ctx.stored,
            facts: ctx.facts,
            table: ctx.table,
            station: ctx.station,
            scanner: ctx.scanner,
            data_root: ctx.data_root,
            capture_root: ctx.capture_root,
            bind: ctx.bind,
            law: ctx.law,
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
                    "Feasibility exceeded: the section at full level is over its kinematic \
                     ceilings. Review it, or arm with an acknowledgement.",
                )
                .with_detail(json!({
                    "feasibility": {"verdict": feasibility.0, "message": feasibility.1}
                })))
            }
            ArmOutcome::Armed(a) => {
                let a = *a;
                if let Some(ack) = &ack {
                    self.shared
                        .add_log("override", format!("operator acknowledgement: {ack}"));
                }
                self.view.feasibility = Some(a.feasibility.clone());
                self.view.grid = Grid {
                    fs_hz: a.fs_hz,
                    record_len: a.record_len,
                    section_start: self.view.grid.section_start,
                    section_len: self.view.grid.section_len,
                    actual: true,
                };
                self.view.clock = Some("shared");
                self.view.station = a.station;
                self.view.streaming = true;
                self.view.units = a.units;
                self.data.set_station(a.fs_hz, a.tap);
                self.lock = a.lock;
                self.set(State::Armed, "armed");
                self.shared.add_log(
                    "station",
                    format!(
                        "armed on {} at {} Hz, records of {} samples",
                        self.view.station.class,
                        num(crate::shock::view::sig(a.fs_hz, 9)),
                        a.record_len
                    ),
                );
                Ok(())
            }
        }
    }

    // ---- identification, review, proceed --------------------------------------------

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
                self.data.set_identification(self.engine.identification());
                self.hold_message = message.clone();
                self.set(State::Held, "coherence_hold");
                self.shared.add_log("trip", message);
                self.publish();
            }
            Ok(Identified::Ended { facts, end }) => {
                self.view.identification = facts;
                self.end_engine(end);
            }
            Ok(Identified::Done { facts, review }) => {
                self.view.identification = Some(facts);
                self.data.set_identification(self.engine.identification());
                self.data.set_mask(self.engine.mask());
                self.data.set_target(self.engine.level_target());
                if review {
                    self.set(State::Review, "awaiting_review");
                    self.publish();
                } else {
                    self.proceed();
                }
            }
        }
    }

    fn proceed(&mut self) {
        let Some(r) = self.call("the law's seed", |e| e.proceed()) else {
            return;
        };
        if self.interrupted() {
            return;
        }
        match r {
            Err(e) => self.end("fault", None, Some(e.message)),
            Ok(Proceeded::Ended(end)) => self.end_engine(end),
            Ok(Proceeded::Ready(prediction)) => {
                self.view.prediction = prediction;
                self.seeded = true;
                self.set(State::Running, "waiting");
                match prediction {
                    Some(p) => self.shared.add_log(
                        "note",
                        format!(
                            "seeded; the drive in force peaks at {} V at {}",
                            num(crate::shock::view::sig(p.drive_vpk, 4)),
                            level_words(p.level_db)
                        ),
                    ),
                    None => self.shared.add_log("note", "seeded"),
                }
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

    // ---- between plays and playing -----------------------------------------------

    /// Wait between plays: take requests, and when a play is due (in `auto`, or
    /// through a playback `twr.play` began) play it. False when the service has
    /// dropped the run.
    fn between_plays(&mut self, rx: &Receiver<Cmd>) -> bool {
        let pos = self.engine.position();
        let due =
            self.seeded && !pos.holding && (self.plan.cfg.mode == TwrRunMode::Auto || pos.playing);
        if !due {
            match rx.recv_timeout(rails::POLL) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return false,
            }
            if self.view.state.is_live() {
                let _ = self.interrupted();
            }
            return true;
        }
        let signals = self.shared.signals.clone();
        let mut gone = false;
        let waited = rails::wait_until(Instant::now(), &signals, || loop {
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
            Waited::Due => {
                if self.view.state == State::Running {
                    self.play_once();
                }
            }
        }
        true
    }

    fn may_iterate(&self) -> Result<(), MethodError> {
        if self.plan.cfg.mode == TwrRunMode::Auto {
            return Err(wrong_mode("An iteration"));
        }
        if self.view.state == State::Held {
            return Err(coherence_hold(&self.hold_message));
        }
        let pos = self.engine.position();
        if !(self.view.state == State::Running
            && self.view.phase == "waiting"
            && self.seeded
            && !pos.playing)
        {
            return Err(wrong_state(&self.view, "An iteration"));
        }
        Ok(())
    }

    fn play_cmd(&mut self) -> Result<(), MethodError> {
        if self.plan.cfg.mode == TwrRunMode::Auto {
            return Err(wrong_mode("Playback"));
        }
        if self.view.state == State::Held {
            return Err(coherence_hold(&self.hold_message));
        }
        let pos = self.engine.position();
        if !(self.view.state == State::Running
            && matches!(self.view.phase, "waiting" | "holding")
            && self.seeded
            && !pos.playing)
        {
            return Err(wrong_state(&self.view, "Playback"));
        }
        if !pos.gated {
            return Err(not_converged(pos.level_db));
        }
        let Some(r) = self.call("beginning playback", |e| e.begin_playback()) else {
            return Err(MethodError::internal("the run failed beginning playback"));
        };
        r?;
        self.shared.add_log(
            "note",
            format!("playback begins at {}", level_words(pos.level_db)),
        );
        Ok(())
    }

    fn play_once(&mut self) {
        let pos = self.engine.position();
        let index = self.data.row_count();
        let starting = Starting {
            index,
            playback: pos.playing,
            iteration: (!pos.playing).then_some(pos.at_level + 1),
        };
        self.data.start_play(starting, self.engine.frames_clocked());
        self.set(
            State::Running,
            if pos.playing { "playing" } else { "iterating" },
        );
        self.publish();
        let r = self.call("a play", |e| e.play());
        self.data.end_play(self.engine.frames_clocked());
        let Some(r) = r else {
            return;
        };
        let played = match r {
            Ok(p) => p,
            Err(e) => {
                self.end("fault", None, Some(e.message));
                return;
            }
        };
        if let Some(done) = played.cancelled_at {
            self.cancelled = Some((index, done));
            if !self.interrupted() {
                self.end(
                    "fault",
                    None,
                    Some("A play was cancelled with no stop asked for.".into()),
                );
            }
            return;
        }
        if let Some(row) = played.row {
            self.view.plays.done += 1;
            self.view.last_play = Some(row.clone());
            self.data.add_play(row, played.records);
            self.data.set_target(self.engine.level_target());
        }
        if let Some(end) = played.end {
            self.end_engine(end);
            return;
        }
        if self.interrupted() {
            return;
        }
        let holding = self.engine.position().holding;
        self.set(State::Running, if holding { "holding" } else { "waiting" });
        self.publish();
    }

    fn level_cmd(&mut self, to: LevelMove) -> Result<f64, MethodError> {
        if self.plan.cfg.mode == TwrRunMode::Auto {
            return Err(wrong_mode("A level change"));
        }
        let pos = self.engine.position();
        let live = matches!(
            (self.view.state, self.view.phase),
            (State::Running, "waiting") | (State::Running, "holding")
        ) && !pos.playing;
        if !live {
            return Err(wrong_state(&self.view, "A level change"));
        }
        let levels = &self.plan.cfg.levels_db;
        let (lo, hi) = (
            levels.first().copied().unwrap_or(0.0),
            levels.last().copied().unwrap_or(0.0),
        );
        let target = match to {
            LevelMove::Step(d) => pos.level_db + d,
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
                    num(crate::shock::config::round(target, 2)),
                    num(lo),
                    num(hi)
                ),
            )
            .with_detail(
                json!({"levels_db": levels.iter().map(|l| num(*l)).collect::<Vec<_>>()}),
            ));
        }
        if let Some(vpk) = self.engine.predicted_vpk_at(target) {
            let ceiling = self.plan.cfg.drive_vpk_ceiling;
            if vpk > ceiling {
                return Err(refuse(
                    "drive_ceiling",
                    format!(
                        "The drive in force would need {} V pk at {}; the drive ceiling is {} V \
                         pk. The level and the drive are unchanged.",
                        num(crate::shock::view::sig(vpk, 4)),
                        level_words(target),
                        num(ceiling)
                    ),
                )
                .with_detail(json!({
                    "vpk": num(crate::shock::view::sig(vpk, 4)),
                    "ceiling_vpk": num(ceiling),
                    "level_db": num(crate::shock::config::round(target, 2)),
                })));
            }
        }
        let Some(r) = self.call("a level change", |e| e.set_level(to)) else {
            return Err(MethodError::internal("the run failed changing level"));
        };
        let level = r?;
        self.data.set_target(self.engine.level_target());
        if self.view.phase == "holding" {
            self.set(State::Running, "waiting");
        }
        self.shared
            .add_log("level", format!("level {}", level_words(level)));
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
            "completed" | "operator_stop" | "safe_stop" => "stopped",
            _ => "aborted",
        };
        let message = message.unwrap_or_else(|| outcome.replace('_', " "));
        let fs = self.view.grid.fs_hz;
        let at_s =
            (self.view.grid.actual && fs > 0.0).then(|| self.engine.frames_clocked() as f64 / fs);
        let (play, frames_done) = match self.cancelled {
            Some((i, done)) => (Some(i), Some(done)),
            None => (
                (self.view.plays.done > 0).then(|| self.view.plays.done - 1),
                None,
            ),
        };
        self.view.end = Some(End {
            outcome,
            message: message.clone(),
            at_s,
            level_db: self.view.level_db,
            play,
            frames_done,
            trip,
        });
        self.set(State::Finished, phase);
        self.shared
            .add_log("state", format!("ended ({outcome}): {message}"));
        self.publish();
        self.lock = None;
    }
}
