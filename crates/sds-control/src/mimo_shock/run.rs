//! One configured MIMO Shock run, on its own thread, on the transient-run rails
//! ([`crate::pulse::rails`]).
//!
//! The thread owns the run's engine ([`hertzforge_mimo_shock::MimoShockEngine`])
//! outright and drives it in the protocol's order: arm, identification, the
//! review (and the unreachable hold), the seed, then pulses (in `auto` at the
//! time between pulses, in `manual` on `mimo_shock.fire`), and the end. Nothing
//! here is control math. Requests arrive as [`Cmd`]s the thread takes between
//! pulses. Stop and Abort reach a fire in flight through the run's cancel flag.
//! An absent controller raises the hold: no pulse starts while it stands. Every
//! engine call runs under `catch_unwind`; a panic ends the run in the emergency
//! stop. At the end the station is released, then the hardware lock, and only
//! then is the end published.

use super::config::WireMs;
use super::view::{
    ControlFigures, End, IdentFigures, LawReading, MsView, PulseFigures, SrsFigures,
};
use super::RunData;
use crate::protocol::{num, ErrorCode, MethodError};
use crate::pulse::rails::{self, guarded, Shared, Waited};
use crate::random::view::State;
use crate::station::{NiScanner, StationPlan};
use hertzforge_channel_table::ChannelTable;
use hertzforge_law_host::DiagnosticSpec;
use hertzforge_mimo_shock::judge::delayed;
use hertzforge_mimo_shock::{
    EndOutcome, EngineError, MimoShockEngine, MimoShockLawFactory, PulseReport, RunState, Target,
};
use hertzforge_mimo_shock_laws::HostRule;
use hertzforge_shock_common::ShockRunMode;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// Who stops a run when the service shuts down.
pub const SHUTDOWN_BY: &str = crate::random::run::SHUTDOWN_BY;

/// The rate a simulator station states, Hz: the run asks its own rate of it.
pub const SIM_STATED_RATE_HZ: f64 = 51_200.0;

type Reply<T> = oneshot::Sender<Result<T, MethodError>>;

/// What a run arms on, taken from the service when the request arrives.
pub struct ArmContext {
    pub table: ChannelTable,
    pub station: StationPlan,
    pub scanner: Arc<dyn NiScanner>,
    pub data_root: PathBuf,
    pub capture_root: Option<PathBuf>,
    pub bind: String,
    pub law: Result<Arc<dyn MimoShockLawFactory>, MethodError>,
    pub host_rule: Option<HostRule>,
    /// The values the law reports, labelled.
    pub law_values: Vec<DiagnosticSpec>,
}

/// A request to the run thread.
pub enum Cmd {
    Arm {
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
    Fire {
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
    pub cfg: Arc<WireMs>,
    pub record_len: usize,
    /// The orphan timeout, for the safe stop's message.
    pub timeout_s: f64,
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

pub fn wrong_mode(verb: &str) -> MethodError {
    refuse(
        "wrong_mode",
        format!("{verb} needs the manual ladder; this run is automatic."),
    )
    .with_detail(json!({"mode": "auto"}))
}

pub fn not_yet(seconds: f64) -> MethodError {
    refuse(
        "not_yet",
        format!(
            "The next pulse may fire in {} s, when the time between pulses has passed.",
            num(super::view::r(seconds.max(0.0), 2))
        ),
    )
    .with_detail(json!({"seconds": num(super::view::r(seconds.max(0.0), 3))}))
}

/// The words of a list of channels: `1`, `1 and 3`, `1, 2 and 3`.
fn and_list(xs: &[String]) -> String {
    match xs.len() {
        0 => String::new(),
        1 => xs[0].clone(),
        n => format!("{} and {}", xs[..n - 1].join(", "), xs[n - 1]),
    }
}

/// The unreachable hold's message.
pub fn unreachable_message(channels: &[u32], floors_db: &[f64]) -> String {
    let ch: Vec<String> = channels.iter().map(|c| c.to_string()).collect();
    let fl: Vec<String> = floors_db
        .iter()
        .map(|f| num(super::view::r(*f, 1)).to_string())
        .collect();
    if ch.len() == 1 {
        format!(
            "Control {} cannot reach its target: its best reachable response is {} dB from it, \
             outside the tolerance. Proceed with an acknowledgement, or judge the run by the \
             global verdict.",
            ch[0], fl[0]
        )
    } else {
        format!(
            "Controls {} cannot reach their targets: their best reachable responses are {} dB \
             from them, outside the tolerance. Proceed with an acknowledgement, or judge the run \
             by the global verdict.",
            and_list(&ch),
            and_list(&fl)
        )
    }
}

fn outcome_name(o: EndOutcome) -> &'static str {
    match o {
        EndOutcome::Completed => "completed",
        EndOutcome::OperatorStop => "operator_stop",
        EndOutcome::OperatorAbort => "operator_abort",
        EndOutcome::SafeStop => "safe_stop",
        EndOutcome::Aborted => "aborted",
        EndOutcome::NotConverged => "not_converged",
        EndOutcome::Held => "held",
        EndOutcome::LawFault => "law_fault",
        EndOutcome::Fault => "fault",
    }
}

/// The run thread's state.
struct Actor {
    plan: RunPlan,
    shared: Arc<Shared<MsView>>,
    data: Arc<RunData>,
    engine: Option<MimoShockEngine>,
    view: MsView,
    lock: Option<hertzforge_station::lock::LockGuard>,
    /// Where the run's records go when it ends: the records root and the folder
    /// name asked for. The folder is claimed then, as the first free one.
    capture_to: Option<(PathBuf, String)>,
    host_rule: Option<HostRule>,
    law_values: Vec<DiagnosticSpec>,
    started: Option<Instant>,
    armed_at: Option<Instant>,
    ended_at: Option<f64>,
    last_start: Option<Instant>,
    seeded: bool,
    stop_outcome: &'static str,
    stopped_by: String,
}

/// Start the run thread. Returns once the thread exists; the run is `configured`.
pub fn spawn(
    plan: RunPlan,
    initial: MsView,
    shared: Arc<Shared<MsView>>,
    data: Arc<RunData>,
    rx: Receiver<Cmd>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let name = format!("mimo-shock-{}", plan.run_id);
    std::thread::Builder::new().name(name).spawn(move || {
        let mut a = Actor {
            plan,
            shared,
            data,
            engine: None,
            view: initial,
            lock: None,
            capture_to: None,
            host_rule: None,
            law_values: Vec::new(),
            started: None,
            armed_at: None,
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
            if let Some(e) = self.engine.as_mut() {
                let _ = guarded(|| e.abort());
            }
        }
        self.engine = None;
        self.lock = None;
    }

    fn publish(&mut self) {
        if let Some(t0) = self.started {
            self.view.elapsed_s = Some(self.ended_at.unwrap_or_else(|| t0.elapsed().as_secs_f64()));
        }
        self.position();
        self.shared.publish(&self.plan.run_id, self.view.clone());
    }

    /// The level, the rung and the counts from the engine.
    fn position(&mut self) {
        let Some(e) = self.engine.as_ref() else {
            return;
        };
        let level = e
            .next_level_db()
            .or_else(|| e.pulses().last().map(|p| p.level_db));
        if let Some(l) = level {
            self.view.level_db = Some(l);
            self.view.rung = self
                .view
                .levels_db
                .iter()
                .position(|x| (x - l).abs() < 1e-9);
            self.view.pulses.at_level = e
                .pulses()
                .iter()
                .filter(|p| (p.level_db - l).abs() < 1e-9)
                .count() as u32;
        }
        self.view.pulses.fired = e.pulses().len() as u64;
        self.view.pulses.passed = e.pulses().iter().filter(|p| p.passed).count() as u64;
        self.view.pulses.of_level = self.plan.cfg.max_pulses_per_level as u32;
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
    fn call<T>(&mut self, what: &str, f: impl FnOnce(&mut MimoShockEngine) -> T) -> Option<T> {
        let engine = self.engine.as_mut()?;
        match guarded(|| f(engine)) {
            Ok(t) => Some(t),
            Err(words) => {
                self.shared.signals.raise(true);
                if let Some(e) = self.engine.as_mut() {
                    let _ = guarded(|| e.abort());
                }
                self.end(
                    "fault",
                    "fault".into(),
                    format!("The run stopped on a fault in {what}: {words}"),
                );
                None
            }
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Arm { ctx, reply } => {
                let r = self.arm_cmd(ctx);
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
            Cmd::Fire { reply } => match self.may_fire("A fire") {
                Err(e) => {
                    let _ = reply.send(Err(e));
                }
                Ok(()) => {
                    let _ = reply.send(Ok(()));
                    self.fire();
                }
            },
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

    fn arm_cmd(&mut self, ctx: ArmContext) -> Result<(), MethodError> {
        match self.view.state {
            State::Configured => {}
            State::Finished => return Err(self.finished_refusal()),
            _ => return Err(self.run_active()),
        }
        self.arm(ctx)
    }

    fn start_cmd(&mut self, ctx: ArmContext) -> Result<(), MethodError> {
        match self.view.state {
            State::Configured => self.arm(ctx)?,
            State::Armed => {}
            State::Finished => return Err(self.finished_refusal()),
            _ => return Err(self.run_active()),
        }
        self.started = Some(Instant::now());
        self.set(State::Running, "sysid");
        self.shared.add_log(
            "note",
            format!(
                "started; identification at up to {} V rms on every drive",
                num(self.plan.cfg.drive_vrms)
            ),
        );
        Ok(())
    }

    fn arm(&mut self, ctx: ArmContext) -> Result<(), MethodError> {
        let law = ctx.law?;
        let cfg = self.plan.cfg.clone();
        let built = crate::station::build(&ctx.station, &*ctx.scanner)?;
        let hardware = built.station.is_hardware();
        let lock = if hardware {
            Some(
                hertzforge_station::lock::acquire(
                    &ctx.data_root,
                    &ctx.bind,
                    &built.provenance.station_label,
                )
                .map_err(|e| match e {
                    hertzforge_station::lock::LockError::Held(h) => MethodError::named(
                        ErrorCode::Refused,
                        "hardware_busy",
                        format!("Another program holds the NI hardware: {}", h.advice()),
                    )
                    .with_detail(json!({"holder": h.advice()})),
                    hertzforge_station::lock::LockError::Io(m) => refuse("arm_refused", m),
                })?,
            )
        } else {
            None
        };
        let class = built.station;
        let provenance = built.provenance.clone();
        let engine = MimoShockEngine::new(cfg.engine_config(self.plan.record_len), law)
            .map_err(|e| refuse("arm_refused", format!("The run could not be built: {e}.")))?
            .with_target_spec(cfg.target_spec())
            .with_pacing(hardware);
        let mut engine = engine;
        engine
            .arm(Box::new(built.backend), ctx.table, class)
            .map_err(|e| {
                refuse(
                    "arm_refused",
                    format!("The station could not be armed: {e}."),
                )
            })?;
        self.view.fs_hz = engine.fs_hz();
        self.view.actual = true;
        self.view.clock = Some("shared");
        self.view.station = crate::shock::view::StationRef {
            kind: if matches!(ctx.station, StationPlan::Sim { .. }) {
                "sim"
            } else {
                "ni"
            },
            class: crate::station::class_name(class),
            devices: provenance
                .device_ids
                .iter()
                .cloned()
                .zip(provenance.serials.iter().cloned())
                .collect(),
        };
        if !matches!(ctx.station, StationPlan::Sim { .. }) {
            self.view.plant = None;
        }
        self.view.streaming = true;
        self.lock = lock;
        self.host_rule = ctx.host_rule;
        self.law_values = ctx.law_values;
        self.capture_to = ctx
            .capture_root
            .filter(|_| cfg.capture_auto_start)
            .map(|root| (root, format!("{}_{}", cfg.run_name, self.plan.run_id)));
        self.engine = Some(engine);
        self.armed_at = Some(Instant::now());
        self.set(State::Armed, "armed");
        self.shared.add_log(
            "station",
            format!(
                "armed on {} at {} Hz (asked {} Hz), records of {} samples, {} drives, {} \
                 controls",
                self.view.station.class,
                num(super::view::r(self.view.fs_hz, 3)),
                num(cfg.fs_hz),
                self.plan.record_len,
                cfg.drives.len(),
                cfg.controls.len()
            ),
        );
        Ok(())
    }

    // ---- identification, review, seed ------------------------------------------

    fn identify(&mut self) {
        self.publish();
        let Some(r) = self.call("identification", |e| e.identify()) else {
            return;
        };
        if self.interrupted() {
            return;
        }
        if let Err(e) = r {
            self.end_engine_or("fault", format!("Identification failed: {e}."));
            return;
        }
        self.view.identification = self.identification_json();
        self.data.set_identification(self.ident_figures());
        if self.engine.as_ref().is_some_and(|e| e.end().is_some()) {
            self.end_engine();
            return;
        }
        self.view.review = self.review_json();
        let held = self.engine.as_ref().and_then(|e| e.unreachable()).is_some();
        if held {
            self.shared.add_log(
                "note",
                "a control cannot reach its target; the run holds before the first pulse",
            );
        }
        if self.plan.cfg.review || held {
            self.set(State::Review, "awaiting_review");
            self.publish();
        } else {
            self.seed();
        }
    }

    fn proceed_cmd(&mut self, ack: Option<String>) -> Result<(), MethodError> {
        if self.view.state != State::Review {
            return Err(wrong_state(&self.view, "Proceeding"));
        }
        let held = self.engine.as_ref().and_then(|e| e.unreachable());
        if let Some(held) = held {
            match ack {
                Some(words) => {
                    self.shared.add_log(
                        "override",
                        format!("operator acknowledged the unreachable targets: {words}"),
                    );
                    if let Some(e) = self.engine.as_mut() {
                        e.acknowledge_unreachable(&words);
                    }
                }
                None => {
                    let e = self.engine.as_ref().expect("armed");
                    let sr = e.seed_report().expect("identified");
                    let chans: Vec<u32> = held.iter().map(|&c| self.plan.cfg.controls[c]).collect();
                    let floors: Vec<f64> = held.iter().map(|&c| sr.floor_db[c]).collect();
                    return Err(refuse(
                        "unreachable_target",
                        unreachable_message(&chans, &floors),
                    )
                    .with_detail(json!({
                        "controls": self.view.review.as_ref().map_or(Value::Null, |r| r["controls"].clone()),
                    })));
                }
            }
        }
        self.shared
            .add_log("note", "the operator reviewed identification and proceeded");
        self.seed();
        Ok(())
    }

    fn seed(&mut self) {
        let Some(r) = self.call("the law's seed", |e| e.proceed()) else {
            return;
        };
        if self.interrupted() {
            return;
        }
        match r {
            Err(EngineError::Unreachable { .. }) => {
                self.set(State::Review, "awaiting_review");
                self.publish();
                return;
            }
            Err(e) => {
                self.end_engine_or("fault", format!("The law could not be seeded: {e}."));
                return;
            }
            Ok(()) => {}
        }
        if self.engine.as_ref().is_some_and(|e| e.end().is_some()) {
            self.end_engine();
            return;
        }
        self.view.review = self.review_json();
        self.seeded = true;
        self.set(State::Running, "waiting");
        self.shared.add_log(
            "note",
            format!(
                "seeded at {} dB",
                num(self
                    .engine
                    .as_ref()
                    .and_then(|e| e.next_level_db())
                    .unwrap_or(0.0))
            ),
        );
        self.publish();
    }

    /// Ended by the operator's signal while a long call ran.
    fn interrupted(&mut self) -> bool {
        if !self.view.state.is_live() {
            return true;
        }
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

    // ---- between pulses and firing ---------------------------------------------

    fn spacing(&self) -> Duration {
        Duration::from_secs_f64(self.plan.cfg.spacing_s())
    }

    fn next_due(&self) -> Instant {
        self.last_start
            .map_or_else(Instant::now, |t| t + self.spacing())
    }

    /// Wait between pulses: take requests, and in `auto` fire when the next pulse
    /// is due. False when the service has dropped the run.
    fn between_pulses(&mut self, rx: &Receiver<Cmd>) -> bool {
        let auto = self.plan.cfg.mode == ShockRunMode::Auto;
        if !auto || !self.seeded {
            self.data.set_next_due(self.seeded.then(|| self.next_due()));
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
            Waited::Due => self.fire(),
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

    fn fire(&mut self) {
        if self.shared.signals.held() {
            return;
        }
        self.last_start = Some(Instant::now());
        self.data.set_next_due(None);
        self.set(State::Running, "firing");
        self.publish();
        let cancel = self.shared.signals.cancel.clone();
        let Some(r) = self.call("a pulse", |e| e.fire_next(Some(&cancel))) else {
            return;
        };
        let report = match r {
            Ok(p) => p,
            Err(e) => {
                self.end_engine_or("fault", format!("The pulse failed: {e}."));
                return;
            }
        };
        let fired = self
            .engine
            .as_ref()
            .is_some_and(|e| e.pulses().last().is_some_and(|p| p.index == report.index));
        if fired {
            let (row, figures) = self.row_and_figures(&report);
            self.view.last_pulse = Some(row.clone());
            self.data.add_pulse(row, figures);
            self.data.set_law(self.law_readings(&report));
            self.shared.add_log(
                "pulse",
                format!(
                    "pulse {} at {} dB: {}, worst over-test {} dB",
                    report.index + 1,
                    num(super::view::r(report.level_db, 2)),
                    if report.passed { "passed" } else { "failed" },
                    num(super::view::r(report.over_test_db, 2))
                ),
            );
        }
        if self.engine.as_ref().is_some_and(|e| e.end().is_some()) {
            if self.shared.signals.aborting() {
                self.end_on_abort();
            } else if self
                .engine
                .as_ref()
                .and_then(|e| e.end())
                .is_some_and(|e| e.reason == "cancelled")
            {
                self.end_on_stop();
            } else {
                self.end_engine();
            }
            return;
        }
        if self.interrupted() {
            return;
        }
        self.set(State::Running, "waiting");
        self.publish();
    }

    // ---- what a pulse shows ------------------------------------------------------

    fn law_readings(&self, report: &PulseReport) -> Vec<LawReading> {
        report
            .law_values
            .iter()
            .map(|v| {
                let spec = self.law_values.iter().find(|s| s.key == v.key);
                LawReading {
                    label: spec.map_or_else(|| v.key.clone(), |s| s.label.clone()),
                    value: super::view::r(v.value, 6),
                    unit: spec.map_or_else(String::new, |s| s.unit.clone()),
                }
            })
            .collect()
    }

    fn row_and_figures(&self, p: &PulseReport) -> (Value, PulseFigures) {
        let e = self.engine.as_ref().expect("armed");
        let cfg = &self.plan.cfg;
        let fs = e.fs_hz();
        let n = cfg_record_len(e);
        let s = 10f64.powf(p.level_db / 20.0);
        let target = e.target().cloned();
        let entry = e
            .records()
            .pulses
            .iter()
            .find(|q| q.index == p.index)
            .cloned();
        let call = e.records().law_calls.get(p.index as usize).cloned();
        let amp_tol = match &cfg.target {
            super::config::WireTargetSet::Waveform { standard, .. } => {
                Some(standard.params().amp_tol_frac_of_peak)
            }
            _ => None,
        };
        let mut figs = Vec::new();
        let mut controls = Vec::new();
        for (c, cr) in p.controls.iter().enumerate() {
            let measured: Vec<f64> = entry
                .as_ref()
                .and_then(|q| {
                    let slot = q.channels.iter().position(|&x| x == cr.channel)?;
                    let row = q.rows.get(slot)?;
                    let a = q.quiet_frames.min(row.len());
                    Some(row[a..(a + n).min(row.len())].to_vec())
                })
                .unwrap_or_default();
            let r: Vec<f64> = target
                .as_ref()
                .map(|t| t.waveforms()[c * n..(c + 1) * n].to_vec())
                .unwrap_or_default();
            let moved: Vec<f64> = delayed(&r, fs, cr.verdict.delay_s)
                .iter()
                .map(|v| s * v)
                .collect();
            let band = amp_tol.map(|tol| {
                let pk = s * r.iter().fold(0.0f64, |a, v| a.max(v.abs()));
                (
                    moved.iter().map(|v| v + tol * pk).collect::<Vec<_>>(),
                    moved.iter().map(|v| v - tol * pk).collect::<Vec<_>>(),
                )
            });
            let mut srs_json = Value::Null;
            let srs = match &target {
                Some(Target::Srs(t)) if !measured.is_empty() => {
                    let nf = t.natural_hz.len();
                    let m = hertzforge_dsp::srs::srs_analyze(&measured, fs, &t.natural_hz, t.q);
                    let tp: Vec<f64> = t.points[c * nf..(c + 1) * nf]
                        .iter()
                        .map(|v| s * v)
                        .collect();
                    let errs: Vec<f64> = m
                        .iter()
                        .zip(&tp)
                        .map(|(a, b)| 20.0 * (a.max(1e-300) / b).log10())
                        .collect();
                    let (wi, we) = errs.iter().enumerate().fold((0, 0.0f64), |(i, w), (j, e)| {
                        if e.abs() > w.abs() {
                            (j, *e)
                        } else {
                            (i, w)
                        }
                    });
                    let passed = errs
                        .iter()
                        .filter(|e| **e >= t.tol_lo_db && **e <= t.tol_hi_db)
                        .count();
                    srs_json = json!({
                        "worst_err_db": num(super::view::r(we, 2)),
                        "worst_band_hz": num(super::view::r(t.natural_hz[wi], 2)),
                        "bands": nf, "passed_bands": passed,
                    });
                    Some(SrsFigures {
                        freqs_hz: t.natural_hz.clone(),
                        hi: tp
                            .iter()
                            .map(|v| v * 10f64.powf(t.tol_hi_db / 20.0))
                            .collect(),
                        lo: tp
                            .iter()
                            .map(|v| v * 10f64.powf(t.tol_lo_db / 20.0))
                            .collect(),
                        target: tp,
                        measured: m,
                    })
                }
                _ => None,
            };
            let mut reasons = Vec::new();
            if !cr.shape_ok {
                reasons.push("tolerance");
            }
            if !cr.timing_ok {
                reasons.push("timing");
            }
            if !cr.polarity_ok {
                reasons.push("polarity");
            }
            if !cr.duration_ok {
                reasons.push("duration");
            }
            controls.push(json!({
                "channel": cr.channel,
                "passed": cr.verdict.passed,
                "reasons": reasons,
                "error_db": num(super::view::r(cr.verdict.error_db, 2)),
                "over_test_db": num(super::view::r(cr.over_test_db, 2)),
                "delay_s": num(super::view::r(cr.verdict.delay_s, 6)),
                "peak": super::view::sg(cr.peak, 4),
                "peak_ratio": num(super::view::r(cr.verdict.peak_ratio, 4)),
                "scaled_margin_db": num(super::view::r(cr.scaled_margin_db, 2)),
                "absolute_margin_db": num(super::view::r(cr.absolute_margin_db, 2)),
                "timing_ok": cr.timing_ok,
                "polarity_ok": cr.polarity_ok,
                "duration_ratio": cr.duration_ratio.map_or(Value::Null, |d| num(super::view::r(d, 3))),
                "duration_ok": cr.duration_ok,
                "energy_db": num(super::view::r(cr.energy_db, 2)),
                "rms_duration_ratio": num(super::view::r(cr.rms_duration_ratio, 3)),
                "velocity_ips": super::view::sg(cr.velocity_ips, 4),
                "displacement_pk_pk_in": super::view::sg(cr.displacement_pk_pk_in, 4),
                "predicted_peak_ratio": p.predicted_peak_ratio.get(c)
                    .map_or(Value::Null, |x| num(super::view::r(*x, 4))),
                "srs": srs_json,
            }));
            figs.push(ControlFigures {
                channel: cr.channel,
                unit: "g".into(),
                target: moved,
                band,
                measured,
                srs,
            });
        }
        let drives: Vec<(u32, Vec<f64>)> = cfg
            .drives
            .iter()
            .enumerate()
            .map(|(d, ch)| {
                (
                    ch.channel,
                    entry
                        .as_ref()
                        .map(|q| q.drives[d * n..(d + 1) * n].to_vec())
                        .unwrap_or_default(),
                )
            })
            .collect();
        let row = json!({
            "index": p.index,
            "scheduled_db": num(super::view::r(p.scheduled_db, 2)),
            "level_db": num(super::view::r(p.level_db, 2)),
            "passed": p.passed,
            "at_s": num(super::view::r(
                self.armed_at.map_or(0.0, |t| (self.last_start.unwrap_or(t)).saturating_duration_since(t).as_secs_f64()), 3)),
            "drives": cfg.drives.iter().enumerate().map(|(d, ch)| json!({
                "channel": ch.channel,
                "vpk": num(super::view::r(p.drive_vpk.get(d).copied().unwrap_or(0.0), 4)),
                "limit_scale": num(super::view::r(p.limit_scale.get(d).copied().unwrap_or(1.0), 4)),
            })).collect::<Vec<_>>(),
            "controls": controls,
            "over_test_db": num(super::view::r(p.over_test_db, 2)),
            "global_srs_error_db": p.global_srs_error_db.map_or(Value::Null, |g| num(super::view::r(g, 2))),
            "arrival_spread_s": num(super::view::r(p.arrival_spread_s, 6)),
            "warnings": p.warnings,
            "step_note": p.step_note.as_ref().map_or(Value::Null, |n| json!({
                "from_db": num(super::view::r(n.from_db, 2)),
                "planned_db": num(super::view::r(n.planned_db, 2)),
                "taken_db": num(super::view::r(n.taken_db, 2)),
                "predicted_planned_db": num(super::view::r(n.predicted_planned_db, 2)),
                "predicted_taken_db": num(super::view::r(n.predicted_taken_db, 2)),
                "tolerance_db": num(super::view::r(n.tolerance_db, 2)),
            })),
            "law": {
                "used": p.law_used,
                "miss": p.law_miss,
                "call_ms": call.as_ref().map_or(Value::Null, |c| num(super::view::r(c.call_us as f64 / 1e3, 3))),
                "delays_s": call.as_ref().and_then(|c| c.outputs.as_ref()).and_then(|o| o.delays_s.as_ref())
                    .map_or(Value::Null, |d| Value::from(d.iter().map(|x| num(super::view::r(*x, 6))).collect::<Vec<_>>())),
                "values": p.law_values.iter().map(|v| json!({"key": v.key, "value": num(super::view::r(v.value, 6))})).collect::<Vec<_>>(),
            },
        });
        (
            row,
            PulseFigures {
                fs_hz: fs,
                controls: figs,
                drives,
            },
        )
    }

    fn identification_json(&self) -> Option<Value> {
        let e = self.engine.as_ref()?;
        let i = e.identification()?;
        let cfg = &self.plan.cfg;
        let (lo, hi) = cfg.band(i.fs_hz);
        let n = cfg_record_len(e);
        let band_lines = hertzforge_mimo_shock::ident::band_lines(i.fs_hz, n, lo, hi)
            .iter()
            .filter(|b| **b)
            .count();
        let held = e.end().is_some_and(|x| x.outcome == EndOutcome::Held);
        let rv = |xs: &[f64], d: i32| {
            xs.iter()
                .map(|x| num(super::view::r(*x, d)))
                .collect::<Vec<_>>()
        };
        Some(json!({
            "realizations": i.realizations,
            "drive_vrms": rv(&i.drive_vrms, 4),
            "drive_vpk": rv(&i.drive_vpk, 4),
            "coherence_mean": rv(&i.mean_coherence, 4),
            "coverage": rv(&i.coverage, 4),
            "trusted_lines": i.trusted_lines,
            "band_lines": band_lines,
            "condition_max": num(super::view::r(i.max_condition, 2)),
            "coherence_floor": num(cfg.coherence_floor),
            "passed": !held,
        }))
    }

    fn review_json(&self) -> Option<Value> {
        let e = self.engine.as_ref()?;
        let sr = e.seed_report()?;
        let held = e.unreachable();
        let controls: Vec<Value> = self
            .plan
            .cfg
            .controls
            .iter()
            .enumerate()
            .map(|(c, ch)| {
                json!({
                    "channel": ch,
                    "reachable": num(super::view::r(sr.reachable_fraction.get(c).copied().unwrap_or(f64::NAN), 4)),
                    "floor_db": num(super::view::r(sr.floor_db.get(c).copied().unwrap_or(f64::NAN), 2)),
                    "reachable_ok": sr.reachable_ok.get(c).copied().unwrap_or(false),
                    "predicted_peak_ratio": sr.predicted_peak_ratio.get(c)
                        .map_or(Value::Null, |x| num(super::view::r(*x, 4))),
                })
            })
            .collect();
        let hold = held.map_or(Value::Null, |h| {
            let chans: Vec<u32> = h.iter().map(|&c| self.plan.cfg.controls[c]).collect();
            let floors: Vec<f64> = h.iter().map(|&c| sr.floor_db[c]).collect();
            json!({"reason": "unreachable_target", "message": unreachable_message(&chans, &floors)})
        });
        Some(json!({
            "controls": controls,
            "hold": hold,
            "acknowledged": e.unreachable_acknowledged(),
        }))
    }

    fn ident_figures(&self) -> Option<IdentFigures> {
        let e = self.engine.as_ref()?;
        let s = e.setup()?;
        let n = s.record_len;
        let lines = n / 2 + 1;
        let df = s.fs_hz / n as f64;
        let on: Vec<usize> = (0..lines)
            .filter(|&f| {
                let hz = f as f64 * df;
                hz >= s.band_lo_hz && hz <= s.band_hi_hz && f > 0
            })
            .collect();
        let (nc, nd) = (s.n_controls, s.n_drives);
        let gain = (0..nc)
            .map(|c| {
                (0..nd)
                    .map(|d| {
                        on.iter()
                            .map(|&f| s.plant.frf.line(f)[c * nd + d].norm())
                            .collect()
                    })
                    .collect()
            })
            .collect();
        let coherence = (0..nc)
            .map(|c| {
                on.iter()
                    .map(|&f| s.plant.coherence[c * lines + f])
                    .collect()
            })
            .collect();
        Some(IdentFigures {
            freqs_hz: on.iter().map(|&f| f as f64 * df).collect(),
            controls: self.plan.cfg.controls.clone(),
            drives: self.plan.cfg.drives.iter().map(|d| d.channel).collect(),
            units: vec!["g".into(); nc],
            gain,
            coherence,
            floor: self.plan.cfg.coherence_floor,
        })
    }

    // ---- the end -----------------------------------------------------------------

    fn end_on_stop(&mut self) {
        let by = self
            .shared
            .ended_by()
            .unwrap_or_else(|| self.stopped_by.clone());
        let outcome = self.shared.stop_outcome().unwrap_or(self.stop_outcome);
        let (reason, message) = match outcome {
            "safe_stop" if by == SHUTDOWN_BY => (
                "shutdown",
                "The service was shut down; the run took its controlled stop.".to_owned(),
            ),
            "safe_stop" => (
                "orphan",
                format!(
                    "Nobody held control for {} s; the run took its controlled stop.",
                    num(self.plan.timeout_s.round())
                ),
            ),
            _ => ("operator", format!("Stopped by {by}.")),
        };
        if let Some(e) = self.engine.as_mut() {
            let _ = guarded(|| e.stop());
        }
        self.end(outcome, reason.into(), message);
    }

    fn end_on_abort(&mut self) {
        let by = self
            .shared
            .ended_by()
            .unwrap_or_else(|| self.stopped_by.clone());
        if let Some(e) = self.engine.as_mut() {
            let _ = guarded(|| e.abort());
        }
        self.end(
            "operator_abort",
            "operator".into(),
            format!("Aborted by {by}."),
        );
    }

    /// The engine has ended the run: say how.
    fn end_engine(&mut self) {
        let Some(end) = self.engine.as_ref().and_then(|e| e.end().cloned()) else {
            return;
        };
        let mut reason = end.reason.clone();
        // A law file's refused output the core folds into `wrong_length` is
        // named by the host's own rule.
        if end.outcome == EndOutcome::LawFault && reason == "wrong_length" {
            if let Some(r) = self
                .host_rule
                .as_ref()
                .and_then(|h| h.lock().ok().and_then(|g| *g))
            {
                reason = r.to_owned();
            }
        }
        if end.outcome != EndOutcome::Completed {
            self.shared.add_log("trip", end.detail.clone());
        }
        let message = {
            let d = end.detail.trim();
            let mut c = d.chars();
            match c.next() {
                Some(f) => {
                    let s = format!("{}{}", f.to_uppercase(), c.as_str());
                    if s.ends_with('.') {
                        s
                    } else {
                        format!("{s}.")
                    }
                }
                None => outcome_name(end.outcome).replace('_', " "),
            }
        };
        self.end(outcome_name(end.outcome), reason, message);
    }

    /// End with the engine's own end when it has one, else with `outcome`.
    fn end_engine_or(&mut self, outcome: &'static str, message: String) {
        if self.engine.as_ref().is_some_and(|e| e.end().is_some()) {
            self.end_engine();
        } else {
            if let Some(e) = self.engine.as_mut() {
                let _ = guarded(|| e.abort());
            }
            self.end(outcome, outcome.into(), message);
        }
    }

    /// Record the end. The engine has already stopped and released the station;
    /// the records are written, the hardware lock is dropped, and only then is the
    /// end published.
    fn end(&mut self, outcome: &'static str, reason: String, message: String) {
        if self.view.state == State::Finished {
            return;
        }
        let phase = match outcome {
            "aborted" | "operator_abort" | "fault" | "law_fault" => "aborted",
            "held" => "held",
            _ => "stopped",
        };
        // The station: an engine that has not finished is stopped (it releases
        // the station before it reports the end).
        if let Some(e) = self.engine.as_mut() {
            if e.end().is_none() && !matches!(e.state(), RunState::Idle) {
                let _ = guarded(|| e.stop());
            }
        }
        self.ended_at = self.started.map(|t| t.elapsed().as_secs_f64());
        self.position();
        let pulse = (self.view.pulses.fired > 0).then(|| self.view.pulses.fired - 1);
        let level_db = self
            .engine
            .as_ref()
            .and_then(|e| e.pulses().last().map(|p| p.level_db));
        self.view.end = Some(End {
            outcome,
            reason,
            message: message.clone(),
            at_s: self.ended_at,
            level_db,
            pulse,
        });
        self.view.level_db = level_db.or(self.view.level_db);
        self.view.review = self.review_json().or(self.view.review.take());
        self.data.set_next_due(None);
        if let (Some((root, name)), Some(e)) = (self.capture_to.as_ref(), self.engine.as_ref()) {
            // The run id restarts with each service process, so the name can repeat:
            // the folder is the first free one, never an earlier run's.
            let written = hertzforge_capture::claim_run_dir(root, name)
                .map_err(|e| e.to_string())
                .and_then(|dir| e.records().write_dir(&dir).map(|()| dir));
            match written {
                Ok(dir) => self
                    .shared
                    .add_log("capture", format!("records written to {}", dir.display())),
                Err(m) => self
                    .shared
                    .add_log("capture", format!("records not written: {m}")),
            }
        }
        self.set(State::Finished, phase);
        self.shared
            .add_log("state", format!("ended ({outcome}): {message}"));
        // Release before publishing: the lock goes, then the end is published.
        let mut lock = self.lock.take();
        release_then_publish(&mut lock, || self.publish());
    }
}

fn cfg_record_len(e: &MimoShockEngine) -> usize {
    e.config().record_len
}

/// Drop the hardware lock, then publish: a run started the moment the end
/// arrives finds the station free.
fn release_then_publish(
    lock: &mut Option<hertzforge_station::lock::LockGuard>,
    publish: impl FnOnce(),
) {
    *lock = None;
    publish();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_shock_the_lock_is_free_when_the_end_is_published() {
        let dir = std::env::temp_dir().join(format!("hf_mimo_shock_lock_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut lock =
            Some(hertzforge_station::lock::acquire(&dir, "127.0.0.1:0", "station").unwrap());
        let mut free_at_publish = None;
        release_then_publish(&mut lock, || {
            free_at_publish =
                Some(hertzforge_station::lock::acquire(&dir, "127.0.0.1:0", "station").is_ok());
        });
        assert_eq!(
            free_at_publish,
            Some(true),
            "the lock was held when the end was published"
        );
        assert!(lock.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mimo_shock_the_unreachable_message_names_each_control_and_floor() {
        assert!(unreachable_message(&[2], &[5.04]).starts_with("Control 2 cannot reach its target"));
        let m = unreachable_message(&[1, 2, 3], &[4.0, 5.0, 6.0]);
        assert!(
            m.contains("Controls 1, 2 and 3") && m.contains("4, 5 and 6 dB"),
            "{m}"
        );
    }
}
